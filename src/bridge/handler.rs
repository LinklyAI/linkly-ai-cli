/// StdioBridgeHandler — MCP server that proxies tool calls to the desktop app's HTTP MCP server.
///
/// This allows `linkly mcp` to act as a stdio MCP server that Claude Desktop
/// or other MCP clients can connect to, while transparently forwarding all
/// tool calls to the actual Linkly AI desktop app over HTTP.
use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Content, Implementation, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::client::McpClient;

#[derive(Clone)]
pub struct StdioBridgeHandler {
    client: std::sync::Arc<McpClient>,
    tool_router: ToolRouter<Self>,
}

impl StdioBridgeHandler {
    pub fn new(client: McpClient) -> Self {
        Self {
            client: std::sync::Arc::new(client),
            tool_router: Self::tool_router(),
        }
    }
}

// ── Input types (mirror desktop app's tools.rs) ─────────

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct SearchInput {
    #[schemars(description = "Search keywords or phrases")]
    pub query: String,

    #[serde(default)]
    #[schemars(description = "Maximum number of results to return (default: 20, max: 50)")]
    pub limit: Option<usize>,

    #[serde(default)]
    #[schemars(
        description = "Filter by document types, e.g. [\"pdf\", \"md\", \"docx\", \"txt\", \"html\"]"
    )]
    pub doc_types: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct OutlineInput {
    #[schemars(description = "List of document IDs (obtained from search results)")]
    pub doc_ids: Vec<String>,

    #[serde(default)]
    #[schemars(
        description = "Node IDs to expand (e.g. [\"2\", \"3.1\"]). When provided, only specified nodes are fully expanded; others are collapsed. When omitted, shows as many levels as fit within the budget."
    )]
    pub expand: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ReadInput {
    #[schemars(description = "Document ID (obtained from search results)")]
    pub doc_id: String,

    #[serde(default)]
    #[schemars(description = "Starting line number (1-based, default: 1)")]
    pub offset: Option<usize>,

    #[serde(default)]
    #[schemars(description = "Number of lines to read (default: 200, max: 500)")]
    pub limit: Option<usize>,
}

// ── Tool implementations ────────────────────────────────

#[tool_router]
impl StdioBridgeHandler {
    #[tool(
        name = "search",
        description = "[Workflow step 1 of 3: search → outline → read] Search indexed local documents by keywords or phrases. Returns the most relevant documents with titles, paths, types, and text snippets. After finding target documents, use 'outline' to get summaries in batch, then use 'read' to read specific sections of interest."
    )]
    async fn search(
        &self,
        Parameters(input): Parameters<SearchInput>,
    ) -> Result<CallToolResult, McpError> {
        let args = serde_json::to_value(&input)
            .map_err(|e| McpError::internal_error(format!("Serialize error: {}", e), None))?;

        let content = self
            .client
            .call_tool("search", args)
            .await
            .map_err(|e| McpError::internal_error(format!("Bridge error: {}", e), None))?;

        Ok(CallToolResult::success(vec![Content::text(content)]))
    }

    #[tool(
        name = "outline",
        description = "[Workflow step 2 of 3: search → outline → read] Get metadata and structural outline of one or more documents by their IDs (obtained from search results) in batch. Quickly understand document summaries and decide which parts to read in detail with 'read'. Note: only documents with reliable parsed outlines (e.g. Markdown, DOCX with headings) will show structural outlines; for other documents, use 'read' for line-by-line browsing."
    )]
    async fn outline(
        &self,
        Parameters(input): Parameters<OutlineInput>,
    ) -> Result<CallToolResult, McpError> {
        let args = serde_json::to_value(&input)
            .map_err(|e| McpError::internal_error(format!("Serialize error: {}", e), None))?;

        let content = self
            .client
            .call_tool("outline", args)
            .await
            .map_err(|e| McpError::internal_error(format!("Bridge error: {}", e), None))?;

        Ok(CallToolResult::success(vec![Content::text(content)]))
    }

    #[tool(
        name = "read",
        description = "[Workflow step 3 of 3: search → outline → read] Read content of a document by its ID. Supports line-based pagination: use `offset` to start from a specific line number and `limit` to control how many lines to read. Returns content with line numbers. For long documents, read in chunks by advancing the offset."
    )]
    async fn read(
        &self,
        Parameters(input): Parameters<ReadInput>,
    ) -> Result<CallToolResult, McpError> {
        let args = serde_json::to_value(&input)
            .map_err(|e| McpError::internal_error(format!("Serialize error: {}", e), None))?;

        let content = self
            .client
            .call_tool("read", args)
            .await
            .map_err(|e| McpError::internal_error(format!("Bridge error: {}", e), None))?;

        Ok(CallToolResult::success(vec![Content::text(content)]))
    }
}

#[tool_handler]
impl ServerHandler for StdioBridgeHandler {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            server_info: Implementation {
                name: "linkly-ai".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                ..Default::default()
            },
            instructions: Some(
                "Linkly AI — full-text search, document overview, and reading service for the user's local computer.\n\
                 Workflow: search (find relevant documents) → outline (read summaries) → read (read in detail)\n\
                 1. Use 'search' to find the most relevant documents by keywords or phrases\n\
                 2. Use 'outline' to get document metadata and structural outlines in batch\n\
                 3. Use 'read' to read document content with line-based pagination (offset/limit)"
                    .to_string(),
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}
