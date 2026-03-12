use std::borrow::Cow;

use anyhow::{Result, bail};
use rmcp::model::{CallToolRequestParams, ClientInfo, Content, Implementation, RawContent};
use rmcp::service::RunningService;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::{ClientHandler, RoleClient, ServiceExt};

use crate::connection::ConnectionInfo;

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
        preflight_check(&conn.base_url, conn.auth_header.as_deref(), conn.is_remote).await?;

        let mut config = StreamableHttpClientTransportConfig::with_uri(&*conn.mcp_url);
        if let Some(header) = &conn.auth_header {
            let token = header.strip_prefix("Bearer ").unwrap_or(header);
            config = config.auth_header(token);
        }

        let transport = StreamableHttpClientTransport::from_config(config);
        let service = CliClientHandler.serve(transport).await?;
        Ok(Self { service })
    }

    /// Call a tool by name with JSON arguments, returning the text content.
    pub async fn call_tool(&self, name: &str, args: serde_json::Value) -> Result<String> {
        // Convert Value to JsonObject (Map<String, Value>)
        let arguments = match args {
            serde_json::Value::Object(map) => Some(map),
            _ => bail!("Arguments must be a JSON object"),
        };

        let result = self
            .service
            .call_tool(CallToolRequestParams {
                meta: None,
                name: Cow::Owned(name.to_string()),
                arguments,
                task: None,
            })
            .await?;

        if result.is_error.unwrap_or(false) {
            let msg = extract_text(&result.content);
            bail!("Tool error: {}", msg);
        }

        Ok(extract_text(&result.content))
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
/// Remote mode: GET /mcp/health (tunnel health endpoint, requires auth)
async fn preflight_check(base_url: &str, auth_header: Option<&str>, is_remote: bool) -> Result<()> {
    let health_url = if is_remote {
        format!("{}/mcp/health", base_url)
    } else {
        format!("{}/health", base_url)
    };
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()?;

    let mut req = client.get(&health_url);
    if let Some(header) = auth_header {
        req = req.header("Authorization", header);
    }

    let resp = req.send().await.map_err(|e| {
        if e.is_connect() {
            anyhow::anyhow!(
                "Cannot connect to {}\nMake sure Linkly AI desktop app is running.",
                base_url
            )
        } else if e.is_timeout() {
            anyhow::anyhow!("Connection timed out: {}", base_url)
        } else {
            anyhow::anyhow!("Connection error: {}", e)
        }
    })?;

    match resp.status().as_u16() {
        200..=299 => Ok(()),
        401 => {
            let body = resp.text().await.unwrap_or_default();
            bail!(
                "Authentication failed (401): {}\n\
                 For LAN access: use --token <your-token>\n\
                 For remote access: run `linkly auth set-key <api-key>`",
                body.trim()
            )
        }
        403 => {
            let body = resp.text().await.unwrap_or_default();
            if body.contains("disabled") {
                bail!(
                    "LAN access is disabled (403)\n\
                     Enable it in Linkly AI → Settings → MCP → LAN Access"
                )
            } else {
                bail!("Access denied (403): {}", body.trim())
            }
        }
        code => {
            let body = resp.text().await.unwrap_or_default();
            bail!("Server error (HTTP {}): {}", code, body.trim())
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
