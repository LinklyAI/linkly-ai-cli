use anyhow::Result;
use rmcp::ServiceExt;

use crate::bridge::handler::StdioBridgeHandler;
use crate::client::McpClient;
use crate::connection;

/// Run the MCP stdio bridge.
///
/// This creates a stdio MCP server that Claude Desktop (or any MCP client)
/// can connect to. All tool calls are transparently forwarded upstream.
///
/// The upstream decides what the client can reach, and the three modes are not
/// interchangeable:
/// - default — the local desktop. Local content only; `cloud://` is rejected.
/// - `--endpoint` — a desktop on the LAN. Same content boundary as local.
/// - `--remote` — the cloud gateway, which serves local content through the
///   desktop tunnel **and** linked cloud libraries. This is the only bridge
///   mode that can reach cloud libraries at all.
///
/// `--token` is still not accepted: LAN bearer auth is a per-request concern
/// the bridge does not model, and `--remote` authenticates from the stored
/// credentials instead.
pub async fn run(endpoint: Option<&str>, remote: bool) -> Result<()> {
    let conn = connection::resolve(endpoint, None, remote)?;

    // Connect to the desktop app's MCP server. The bridge is a transparent
    // passthrough — skip the version gate so an old Desktop's "tool not
    // found" surfaces verbatim to whatever upstream client (e.g. Claude
    // Desktop) is using us, instead of being intercepted on the way in.
    let client = McpClient::connect_passthrough(&conn).await?;

    // Create the bridge handler and serve over stdio
    let handler = StdioBridgeHandler::new(client, conn);
    let stdio = (tokio::io::stdin(), tokio::io::stdout());
    let service = handler.serve(stdio).await?;

    // Block until either the client disconnects or the user sends Ctrl+C.
    // Without the signal arm the bridge would still terminate on SIGINT
    // (tokio's default), but it would skip our own shutdown path —
    // explicit handling lets us return cleanly so a parent shell sees
    // exit 0 instead of an aborted process.
    tokio::select! {
        result = service.waiting() => {
            result?;
        }
        _ = tokio::signal::ctrl_c() => {
            eprintln!("linkly mcp: received Ctrl+C, shutting down.");
        }
    }

    Ok(())
}
