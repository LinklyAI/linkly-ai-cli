use std::borrow::Cow;
use std::time::Duration;

use anyhow::{Result, bail};
use rmcp::model::{CallToolRequestParams, ClientInfo, Content, Implementation, RawContent};
use rmcp::service::{RunningService, ServiceError};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::{ClientHandler, RoleClient, ServiceExt};
use crate::connection::{ConnectionInfo, ConnectionMode};
use crate::version_check::check_desktop_version;

/// Structured JSON-RPC error returned by the gateway/server, carrying the
/// `code` / `message` / `data` fields so `--json` output can surface the error
/// code and the `data.guidance` the cloud gateway provides. Non-protocol
/// failures (bad arguments, timeout, transport, tool-level `isError`) stay as
/// plain `anyhow` errors and are rendered as strings by `print_tool_error`.
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

impl From<rmcp::model::ErrorData> for ToolError {
    fn from(error: rmcp::model::ErrorData) -> Self {
        ToolError {
            code: error.code.0,
            message: error.message.to_string(),
            data: error.data,
        }
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

        let mut config = StreamableHttpClientTransportConfig::with_uri(&*conn.mcp_url);
        if let Some(header) = &conn.auth_header {
            let token = header.strip_prefix("Bearer ").unwrap_or(header);
            config = config.auth_header(token);
        }

        let transport = StreamableHttpClientTransport::from_config(config);

        let service = tokio::time::timeout(CONNECT_TIMEOUT, CliClientHandler.serve(transport))
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "Connection timed out after {} seconds. Desktop may be offline or unresponsive.\n\n{}",
                    CONNECT_TIMEOUT.as_secs(),
                    conn.doctor_hint()
                )
            })??;

        Ok(Self { service })
    }

    /// Call a tool by name with JSON arguments, returning the text content.
    pub async fn call_tool(
        &self,
        name: &str,
        args: serde_json::Value,
        conn: &ConnectionInfo,
    ) -> Result<String> {
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
            let msg = extract_text(&result.content);
            bail!("Tool error: {}", msg);
        }

        Ok(extract_text(&result.content))
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
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;

    let mut req = client.get(&health_url);
    if let Some(header) = &conn.auth_header {
        req = req.header("Authorization", header);
    }

    let hint = conn.doctor_hint();

    let resp = req.send().await.map_err(|e| {
        if e.is_connect() {
            let advice = match &conn.mode {
                ConnectionMode::Local => "Make sure Linkly AI Desktop is running on this machine.",
                ConnectionMode::Lan { .. } => "Check that the endpoint is correct and Desktop is running on the target machine.",
                ConnectionMode::Remote => "Check your network connection.",
            };
            anyhow::anyhow!(
                "Cannot connect to Linkly AI at {}.\n{}\n\n{}",
                conn.base_url, advice, hint
            )
        } else if e.is_timeout() {
            anyhow::anyhow!(
                "Connection timed out reaching {}.\n\n{}",
                conn.base_url, hint
            )
        } else {
            anyhow::anyhow!("Connection error: {}\n\n{}", e, hint)
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
                ConnectionMode::Lan { .. } => {
                    "\nCheck your --token value (find it in Desktop > Settings > MCP)."
                }
                ConnectionMode::Remote => {
                    "\nCheck your API key: run 'linkly auth set-key <your-api-key>'.\n\
                     Get your key from https://linkly.ai (Dashboard > API Keys)."
                }
            };
            bail!(
                "Authentication failed (401): {}{}\n\n{}",
                body.trim(), advice, hint
            )
        }
        403 => {
            let body = resp.text().await.unwrap_or_default();
            if body.contains("disabled") {
                bail!(
                    "LAN access is disabled on the target machine.\n\
                     Ask the Desktop owner to enable it: Settings > MCP > LAN Access.\n\n\
                     {}",
                    hint
                )
            } else {
                bail!("Access denied (403): {}\n\n{}", body.trim(), hint)
            }
        }
        code => {
            let body = resp.text().await.unwrap_or_default();
            bail!("Server returned HTTP {}: {}\n\n{}", code, body.trim(), hint)
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
    use super::ToolError;
    use rmcp::model::{ErrorCode, ErrorData};

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
        assert_eq!(err.data, Some(serde_json::json!({ "guidance": ["fix it"] })));
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
