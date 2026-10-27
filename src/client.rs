use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use reqwest::header::{HeaderName, HeaderValue};

use crate::connection::{ConnectionInfo, ConnectionMode, CLIENT_HEADER};
use crate::version_check::check_desktop_version;
use anyhow::{bail, Result};
use rmcp::model::{
    CallToolRequestParams, ClientInfo, ClientJsonRpcMessage, Content, Implementation, RawContent,
};
use rmcp::service::{RunningService, ServiceError};
use rmcp::transport::common::client_side_sse::BoxedSseResponse;
use rmcp::transport::streamable_http_client::{
    StreamableHttpClient, StreamableHttpClientTransportConfig, StreamableHttpError,
    StreamableHttpPostResponse,
};
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::{ClientHandler, RoleClient, ServiceExt};

/// Structured JSON-RPC error returned by the gateway/server, carrying the
/// `code` / `message` / `data` fields so `--json` output can surface the error
/// code and the `data.guidance` the cloud gateway provides. Non-protocol
/// failures (bad arguments, timeout, transport) stay as plain `anyhow` errors,
/// and tool-level `isError` is a [`ToolFailure`]; `print_tool_error` renders
/// both as strings.
#[derive(Debug)]
pub struct ToolError {
    pub code: i32,
    pub message: String,
    pub data: Option<serde_json::Value>,
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)?;
        if let Some(data) = &self.data {
            write!(f, " ({})", data)?;
        }
        Ok(())
    }
}

impl std::error::Error for ToolError {}

/// A tool ran but reported failure (`isError: true` in the call result): the
/// gateway's paywall / Desktop offline / timeout notices, or a Desktop-side
/// tool error. `text` is the result's text content, written for the model to
/// read. Kept apart from transport errors so `linkly mcp` can hand it on as a
/// tool result instead of a protocol error.
#[derive(Debug)]
pub struct ToolFailure {
    pub text: String,
}

impl std::fmt::Display for ToolFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Tool error: {}", self.text)
    }
}

impl std::error::Error for ToolFailure {}

impl From<rmcp::model::ErrorData> for ToolError {
    fn from(error: rmcp::model::ErrorData) -> Self {
        ToolError {
            code: error.code.0,
            message: error.message.to_string(),
            data: error.data,
        }
    }
}

/// Headers added to every MCP request. Today that is only the caller's name.
///
/// An unusable name yields no header rather than an error: the name is a label
/// for the user's access log, and a bad label must not cost them the command.
/// `sanitize_client_name` already keeps the value inside printable ASCII, so
/// `from_str` failing here means something upstream changed — drop it quietly
/// and carry on.
fn client_headers(client_name: Option<&str>) -> HashMap<HeaderName, HeaderValue> {
    let mut headers = HashMap::new();
    if let Some(name) = client_name {
        if let Ok(value) = HeaderValue::from_str(name) {
            headers.insert(HeaderName::from_static(CLIENT_HEADER), value);
        }
    }
    headers
}

/// The HTTP client the MCP transport sends through, plus a caller name that
/// can be filled in after the session is already up.
///
/// `--client` is known before connecting and rides in the transport's fixed
/// headers. The stdio bridge only learns the name of the application it
/// serves from that application's `initialize`, which arrives after the
/// upstream session exists — so that name goes in `caller` and is added to
/// every request from then on. A `--client` header already present wins.
#[derive(Clone)]
struct CallerNamedHttp {
    http: reqwest::Client,
    caller: Arc<OnceLock<HeaderValue>>,
}

impl CallerNamedHttp {
    fn with_caller(
        &self,
        mut headers: HashMap<HeaderName, HeaderValue>,
    ) -> HashMap<HeaderName, HeaderValue> {
        if let Some(name) = self.caller.get() {
            headers
                .entry(HeaderName::from_static(CLIENT_HEADER))
                .or_insert_with(|| name.clone());
        }
        headers
    }
}

impl StreamableHttpClient for CallerNamedHttp {
    // Same error type as the plain reqwest client, so callers that inspect
    // transport failures (HTTP status of a rejected call) see no difference.
    type Error = reqwest::Error;

    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        let headers = self.with_caller(custom_headers);
        self.http
            .post_message(uri, message, session_id, auth_header, headers)
            .await
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), StreamableHttpError<Self::Error>> {
        let headers = self.with_caller(custom_headers);
        self.http
            .delete_session(uri, session_id, auth_header, headers)
            .await
    }

    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        last_event_id: Option<String>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<BoxedSseResponse, StreamableHttpError<Self::Error>> {
        let headers = self.with_caller(custom_headers);
        self.http
            .get_stream(uri, session_id, last_event_id, auth_header, headers)
            .await
    }
}

/// Timeout for MCP session initialization (serve / initialize handshake).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Timeout for individual tool calls (search, read, etc.).
const CALL_TOOL_TIMEOUT: Duration = Duration::from_secs(60);

/// Minimal MCP client handler — we only need to identify ourselves.
#[derive(Clone)]
struct CliClientHandler;

impl ClientHandler for CliClientHandler {
    fn get_info(&self) -> ClientInfo {
        ClientInfo {
            client_info: Implementation {
                name: "linkly-ai-cli".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                ..Default::default()
            },
            ..Default::default()
        }
    }
}

/// MCP client wrapping an rmcp RunningService.
pub struct McpClient {
    service: RunningService<RoleClient, CliClientHandler>,
    /// Shared with the transport's [`CallerNamedHttp`]; see [`Self::name_caller`].
    caller: Arc<OnceLock<HeaderValue>>,
}

impl McpClient {
    /// Connect to the MCP server. Performs a pre-flight health check to
    /// provide clear error messages for auth/network failures, and gates
    /// against an outdated Desktop on the other end so users don't hit
    /// confusing per-tool errors when the CLI uses surface area the
    /// Desktop doesn't yet expose.
    pub async fn connect(conn: &ConnectionInfo) -> Result<Self> {
        Self::connect_inner(conn, true).await
    }

    /// Connect without version gating. Used by the `linkly mcp` stdio
    /// bridge: there the bridge is a transparent passthrough — its job
    /// is to forward whatever the upstream client sends to whatever
    /// Desktop happens to be on the other end. A Desktop-too-old
    /// failure surfaces naturally as a per-tool "tool not found" error
    /// to the upstream client, which is an informative outcome.
    pub async fn connect_passthrough(conn: &ConnectionInfo) -> Result<Self> {
        Self::connect_inner(conn, false).await
    }

    async fn connect_inner(conn: &ConnectionInfo, check_version: bool) -> Result<Self> {
        preflight_check(conn, check_version).await?;
        // `from_config` would use a bare `reqwest::Client` with no User-Agent.
        Self::open_session(conn, crate::http::client_builder().build()?).await
    }

    /// Open the MCP session over `http`, with no pre-flight check. Outside
    /// tests, go through [`Self::connect`] / [`Self::connect_passthrough`].
    pub(crate) async fn open_session(conn: &ConnectionInfo, http: reqwest::Client) -> Result<Self> {
        let mut config = StreamableHttpClientTransportConfig::with_uri(&*conn.mcp_url);
        if let Some(header) = &conn.auth_header {
            let token = header.strip_prefix("Bearer ").unwrap_or(header);
            config = config.auth_header(token);
        }
        config = config.custom_headers(client_headers(conn.client_name.as_deref()));

        let caller = Arc::new(OnceLock::new());
        let transport = StreamableHttpClientTransport::with_client(
            CallerNamedHttp {
                http,
                caller: caller.clone(),
            },
            config,
        );

        let service = tokio::time::timeout(CONNECT_TIMEOUT, CliClientHandler.serve(transport))
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "Connection timed out after {} seconds. Desktop may be offline or unresponsive.\n\n{}",
                    CONNECT_TIMEOUT.as_secs(),
                    conn.doctor_hint()
                )
            })??;

        Ok(Self { service, caller })
    }

    /// Test-only: run the client over any rmcp transport — an in-process pipe
    /// to a fake upstream — so the bridge handlers can be exercised end to end
    /// through real JSON-RPC without an HTTP server.
    #[cfg(test)]
    pub(crate) async fn from_transport<T, E, A>(transport: T) -> Result<Self>
    where
        T: rmcp::transport::IntoTransport<RoleClient, E, A>,
        E: std::error::Error + Send + Sync + 'static,
    {
        let service = CliClientHandler.serve(transport).await?;
        Ok(Self {
            service,
            caller: Arc::new(OnceLock::new()),
        })
    }

    /// Name the application this connection works for, on every request from
    /// now on — for when that name is only learned after connecting (the
    /// stdio bridge learns it from its client's `initialize`). Only the first
    /// name sticks, and a `--client` given at connect time keeps precedence.
    /// A name that cannot be a header value is dropped, as in
    /// [`client_headers`].
    pub fn name_caller(&self, name: &str) {
        if let Ok(value) = HeaderValue::from_str(name) {
            let _ = self.caller.set(value);
        }
    }

    /// Call a tool by name with JSON arguments, returning the text content.
    pub async fn call_tool(
        &self,
        name: &str,
        args: serde_json::Value,
        conn: &ConnectionInfo,
    ) -> Result<String> {
        Ok(extract_text(
            &self.call_tool_content(name, args, conn).await?,
        ))
    }

    /// Call a tool by name with JSON arguments, returning every content block —
    /// images included (`read` with `include_image`). A failed tool call
    /// still comes back as `ToolFailure` carrying its text.
    pub async fn call_tool_content(
        &self,
        name: &str,
        args: serde_json::Value,
        conn: &ConnectionInfo,
    ) -> Result<Vec<Content>> {
        // Convert Value to JsonObject (Map<String, Value>)
        let arguments = match args {
            serde_json::Value::Object(map) => Some(map),
            _ => bail!("Arguments must be a JSON object"),
        };

        let call_result = tokio::time::timeout(
            CALL_TOOL_TIMEOUT,
            self.service.call_tool(CallToolRequestParams {
                meta: None,
                name: Cow::Owned(name.to_string()),
                arguments,
                task: None,
            }),
        )
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "Request timed out after {} seconds. Desktop may have disconnected or the operation is taking too long.\n\n{}",
                CALL_TOOL_TIMEOUT.as_secs(),
                conn.doctor_hint()
            )
        })?;

        // Preserve the gateway's structured JSON-RPC error (code + data.guidance)
        // by wrapping it as ToolError; other transport/protocol errors keep their
        // default rendering.
        let result = match call_result {
            Ok(result) => result,
            Err(ServiceError::McpError(error)) => return Err(ToolError::from(error).into()),
            Err(other) => return Err(other.into()),
        };

        if result.is_error.unwrap_or(false) {
            return Err(ToolFailure {
                text: extract_text(&result.content),
            }
            .into());
        }

        Ok(result.content)
    }

    /// List available tools from the MCP server (used by doctor for round-trip check).
    pub async fn list_tools(&self) -> Result<Vec<String>> {
        let result = self.service.list_tools(None).await?;
        Ok(result.tools.iter().map(|t| t.name.to_string()).collect())
    }

    /// Gracefully close the connection.
    #[allow(dead_code)]
    pub async fn close(mut self) -> Result<()> {
        self.service.close().await?;
        Ok(())
    }
}

/// Minimal /health body shape used to extract the Desktop version. The
/// real /health response carries more fields; we only deserialize what
/// we need here (matching the more detailed `HealthResponse` struct in
/// `commands/status.rs`).
#[derive(serde::Deserialize)]
struct VersionProbe {
    version: Option<String>,
}

/// Pre-flight health check: verifies connectivity and auth before establishing the MCP session.
/// Local mode: GET /health (desktop app health endpoint)
/// Remote mode: GET /mcp/health (tunnel health endpoint, requires auth) — also checks tunnel status
///
/// When `check_version` is true the local-mode 200 branch additionally parses the
/// reported Desktop version and refuses to proceed if the Desktop is older than
/// what this CLI knows how to talk to. Remote mode skips the version check
/// because /mcp/health doesn't surface the upstream Desktop version.
async fn preflight_check(conn: &ConnectionInfo, check_version: bool) -> Result<()> {
    let health_url = if conn.is_remote {
        format!("{}/mcp/health", conn.base_url)
    } else {
        format!("{}/health", conn.base_url)
    };
    let client = crate::http::client_builder()
        .timeout(Duration::from_secs(5))
        .build()?;

    let mut req = client.get(&health_url);
    if let Some(header) = &conn.auth_header {
        req = req.header("Authorization", header);
    }

    let hint = conn.doctor_hint();
    // Appended to every failure below: when a proxy is silently in the path, the
    // HTTP error the user sees may be the proxy's rather than Linkly's.
    let proxy_note = crate::connection::proxy_interception_note(conn)
        .map(|note| format!("\n{}\n", note))
        .unwrap_or_default();

    let resp = req.send().await.map_err(|e| {
        if e.is_connect() {
            let advice = match &conn.mode {
                ConnectionMode::Local => "Make sure Linkly AI Desktop is running on this machine.",
                ConnectionMode::Lan { .. } => "Check that the endpoint is correct and Desktop is running on the target machine.",
                ConnectionMode::Remote => "Check your network connection.",
            };
            anyhow::anyhow!(
                "Cannot connect to Linkly AI at {}.\n{}\n{}\n{}",
                conn.base_url, advice, proxy_note, hint
            )
        } else if e.is_timeout() {
            anyhow::anyhow!(
                "Connection timed out reaching {}.\n{}\n{}",
                conn.base_url, proxy_note, hint
            )
        } else {
            anyhow::anyhow!("Connection error: {}\n{}\n{}", e, proxy_note, hint)
        }
    })?;

    match resp.status().as_u16() {
        200..=299 => {
            let body = resp.text().await.unwrap_or_default();

            // Remote mode: tunnel status is NOT a hard gate here. The gateway
            // serves cloud:// tool calls (and aggregates list_libraries) even
            // when Desktop is offline; local/default calls receive a precise
            // -32000 error from the gateway with reconnect guidance. Gating on
            // tunnel status here would make cloud libraries unreachable whenever
            // Desktop happens to be disconnected (the Cloud KB launch blocker).
            if conn.is_remote {
                return Ok(());
            }

            // Local / LAN mode: gate against an outdated Desktop. We can't do this
            // for remote mode because the tunnel /mcp/health body doesn't carry
            // the upstream Desktop version.
            if check_version {
                if let Ok(probe) = serde_json::from_str::<VersionProbe>(&body) {
                    if let Some(version) = probe.version.as_deref() {
                        if let Err(gap) = check_desktop_version(version) {
                            bail!(
                                "Linkly AI Desktop v{} is older than v{}, which this CLI \
                                 (v{}) requires for {}.\n\
                                 Update Desktop: open Settings → About → Check for Updates, \
                                 or download from https://linkly.ai\n\n{}",
                                gap.actual,
                                gap.required,
                                env!("CARGO_PKG_VERSION"),
                                gap.missing_features,
                                hint
                            );
                        }
                    }
                }
            }

            Ok(())
        }
        401 => {
            let body = resp.text().await.unwrap_or_default();
            let advice = match &conn.mode {
                ConnectionMode::Local => "",
                ConnectionMode::Lan { saved: true, .. } => {
                    "\nThe saved LAN token was rejected. Copy the current token from Desktop > Agent usage > View MCP connection settings\n\
                     and run `linkly auth set-key <token> --lan --endpoint <url>`."
                }
                ConnectionMode::Lan { .. } => {
                    "\nCheck your --token value (find it in Desktop > Agent usage > View MCP connection settings)."
                }
                ConnectionMode::Remote => concat!("\n", crate::key_rejected_hint!()),
            };
            bail!(
                "Authentication failed (401): {}{}\n\n{}",
                body.trim(),
                advice,
                hint
            )
        }
        403 => {
            let body = resp.text().await.unwrap_or_default();
            if body.contains("disabled") {
                bail!(
                    "LAN access is disabled on the target machine.\n\
                     Ask the Desktop owner to turn on \"Allow LAN access\" on the Agent usage page.\n\n\
                     {}",
                    hint
                )
            } else {
                bail!("Access denied (403): {}\n\n{}", body.trim(), hint)
            }
        }
        code => {
            let body = resp.text().await.unwrap_or_default();
            // Most proxies answer with an empty body; a dangling ": " reads
            // like the message got cut off.
            let body = body.trim();
            let detail = if body.is_empty() {
                String::new()
            } else {
                format!(": {}", body)
            };
            bail!(
                "Server returned HTTP {}{}\n{}\n{}",
                code,
                detail,
                proxy_note,
                hint
            )
        }
    }
}

/// Extract text from MCP content blocks.
fn extract_text(content: &[Content]) -> String {
    content
        .iter()
        .filter_map(|block| match &block.raw {
            RawContent::Text(tc) => Some(tc.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::{client_headers, CallerNamedHttp, ToolError};
    use crate::connection::CLIENT_HEADER;
    use reqwest::header::{HeaderName, HeaderValue};
    use rmcp::model::{ErrorCode, ErrorData};
    use std::collections::HashMap;
    use std::sync::{Arc, OnceLock};

    #[test]
    fn a_named_caller_gets_the_header() {
        let headers = client_headers(Some("claude-code"));
        assert_eq!(
            headers
                .get(&HeaderName::from_static(CLIENT_HEADER))
                .map(|v| v.to_str().unwrap()),
            Some("claude-code")
        );
    }

    #[test]
    fn an_unnamed_caller_sends_no_header() {
        assert!(client_headers(None).is_empty());
    }

    #[test]
    fn a_value_that_cannot_be_a_header_is_dropped_rather_than_fatal() {
        // sanitize_client_name should have caught this already; if something
        // slips through, the call still has to go out.
        assert!(client_headers(Some("bad\nvalue")).is_empty());
    }

    fn named(caller: Option<&str>) -> CallerNamedHttp {
        let slot = Arc::new(OnceLock::new());
        if let Some(name) = caller {
            slot.set(HeaderValue::from_str(name).unwrap()).unwrap();
        }
        CallerNamedHttp {
            http: reqwest::Client::new(),
            caller: slot,
        }
    }

    fn client_header(headers: &HashMap<HeaderName, HeaderValue>) -> Option<&str> {
        headers
            .get(&HeaderName::from_static(CLIENT_HEADER))
            .map(|v| v.to_str().unwrap())
    }

    #[test]
    fn a_caller_named_after_connecting_is_added_to_requests() {
        let headers = named(Some("claude-ai")).with_caller(client_headers(None));
        assert_eq!(client_header(&headers), Some("claude-ai"));
    }

    #[test]
    fn a_client_flag_given_at_connect_time_keeps_precedence() {
        let headers = named(Some("claude-ai")).with_caller(client_headers(Some("my-workflow")));
        assert_eq!(client_header(&headers), Some("my-workflow"));
    }

    #[test]
    fn no_caller_name_leaves_the_headers_alone() {
        assert!(named(None).with_caller(client_headers(None)).is_empty());
    }

    #[test]
    fn tool_error_from_mcp_maps_all_fields() {
        let mcp = ErrorData {
            code: ErrorCode(-32602),
            message: "Invalid params".into(),
            data: Some(serde_json::json!({ "guidance": ["fix it"] })),
        };
        let err = ToolError::from(mcp);
        assert_eq!(err.code, -32602);
        assert_eq!(err.message, "Invalid params");
        assert_eq!(
            err.data,
            Some(serde_json::json!({ "guidance": ["fix it"] }))
        );
    }

    #[test]
    fn tool_error_from_mcp_preserves_none_data() {
        let mcp = ErrorData {
            code: ErrorCode(-32000),
            message: "Desktop unavailable".into(),
            data: None,
        };
        let err = ToolError::from(mcp);
        assert_eq!(err.code, -32000);
        assert_eq!(err.data, None);
    }

    #[test]
    fn tool_error_display_without_data() {
        let err = ToolError {
            code: -32000,
            message: "Desktop unavailable".into(),
            data: None,
        };
        assert_eq!(err.to_string(), "-32000: Desktop unavailable");
    }

    #[test]
    fn tool_error_display_with_data() {
        let err = ToolError {
            code: -32602,
            message: "Mixed".into(),
            data: Some(serde_json::json!({ "reason": "x" })),
        };
        let rendered = err.to_string();
        assert!(rendered.starts_with("-32602: Mixed ("), "got: {rendered}");
        assert!(rendered.contains("reason"));
    }
}
