use std::borrow::Cow;
use std::time::Duration;

use anyhow::{Result, bail};
use rmcp::model::{CallToolRequestParams, ClientInfo, Content, Implementation, RawContent};
use rmcp::service::RunningService;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::{ClientHandler, RoleClient, ServiceExt};
use crate::connection::{ConnectionInfo, ConnectionMode, RemoteHealthResponse};

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
    /// provide clear error messages for auth/network failures.
    pub async fn connect(conn: &ConnectionInfo) -> Result<Self> {
        preflight_check(conn).await?;

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

        let result = tokio::time::timeout(
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
        })??;

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

/// Pre-flight health check: verifies connectivity and auth before establishing the MCP session.
/// Local mode: GET /health (desktop app health endpoint)
/// Remote mode: GET /mcp/health (tunnel health endpoint, requires auth) — also checks tunnel status
async fn preflight_check(conn: &ConnectionInfo) -> Result<()> {
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
            // Remote mode: also check tunnel status
            if conn.is_remote {
                let body = resp.text().await.unwrap_or_default();
                if let Ok(health) = serde_json::from_str::<RemoteHealthResponse>(&body) {
                    if health.tunnel.as_deref() == Some("disconnected") {
                        bail!(
                            "Linkly AI Desktop is not connected.\n\
                             Launch Desktop and enable \"MCP Connector\" in Settings > MCP.\n\n\
                             {}",
                            hint
                        );
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
