use std::borrow::Cow;

use anyhow::{Result, bail};
use rmcp::model::{CallToolRequestParams, ClientInfo, Content, Implementation, RawContent};
use rmcp::service::RunningService;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::{ClientHandler, RoleClient, ServiceExt};

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
    /// Connect to the MCP server at the given URL.
    pub async fn connect(mcp_url: &str) -> Result<Self> {
        let transport = StreamableHttpClientTransport::from_uri(mcp_url);
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
