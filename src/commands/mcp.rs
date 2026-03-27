use anyhow::Result;
use rmcp::ServiceExt;

use crate::bridge::handler::StdioBridgeHandler;
use crate::client::McpClient;
use crate::connection;

/// Run the MCP stdio bridge.
///
/// This creates a stdio MCP server that Claude Desktop (or any MCP client)
/// can connect to. All tool calls are transparently forwarded to the
/// Linkly AI desktop app's HTTP MCP server.
///
/// Only supports local and LAN endpoint modes (no --remote/--token).
/// When `--endpoint` is used, the URL is normalized through `connection::resolve`.
pub async fn run(endpoint: Option<&str>) -> Result<()> {
    let conn = connection::resolve(endpoint, None, false)?;

    // Connect to the desktop app's MCP server
    let client = McpClient::connect(&conn).await?;

    // Create the bridge handler and serve over stdio
    let handler = StdioBridgeHandler::new(client, conn);
    let stdio = (tokio::io::stdin(), tokio::io::stdout());
    let service = handler.serve(stdio).await?;

    // Block until the client disconnects
    service.waiting().await?;

    Ok(())
}
