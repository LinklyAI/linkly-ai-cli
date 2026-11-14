use std::time::Duration;

use anyhow::{bail, Result};
use rmcp::ServiceExt;

use crate::bridge::handler::StdioBridgeHandler;
use crate::client::McpClient;
use crate::connection;

/// Run the MCP stdio bridge.
///
/// This creates a stdio MCP server that Claude Desktop (or any MCP client)
/// can connect to. All tool calls are transparently forwarded upstream.
///
/// The upstream decides what the client can reach, and the two modes are not
/// interchangeable:
/// - default — the local desktop. Local content only; `cloud://` is rejected.
/// - `--remote` — the cloud gateway, which serves local content through the
///   desktop tunnel **and** linked cloud libraries. This is the only bridge
///   mode that can reach cloud libraries at all.
///
/// Bridging to a desktop on the LAN is deliberately not supported. The bridge
/// exists for local clients that cannot speak HTTP; a client on another machine
/// connects to Desktop's LAN HTTP endpoint directly (or bridges with
/// `--remote`). `--endpoint` is still parsed so that passing it fails here with
/// that guidance, rather than with a bare 401 from Desktop.
pub async fn run(endpoint: Option<&str>, remote: bool, client: Option<&str>) -> Result<()> {
    if let Some(endpoint) = endpoint {
        bail!("{}", lan_bridge_unsupported(endpoint));
    }
    let conn = connection::resolve(None, None, false, remote, client)?;

    // Connect to the desktop app's MCP server. The bridge is a transparent
    // passthrough — skip the version gate so an old Desktop's "tool not
    // found" surfaces verbatim to whatever upstream client (e.g. Claude
    // Desktop) is using us, instead of being intercepted on the way in.
    let client = McpClient::connect_passthrough(&conn).await?;

    // Advertise `pages` only when the upstream does — it follows the
    // account's Linkly AI Pages switch there, and an older Desktop has none.
    let pages = offers_pages(client.list_tools(), PAGES_PROBE_TIMEOUT).await;

    // Create the bridge handler and serve over stdio
    let handler = StdioBridgeHandler::with_pages(client, conn, pages);
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

/// How long the bridge waits, before it starts, to learn whether the upstream
/// offers `pages`: the client that started the bridge is waiting too.
const PAGES_PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Whether the upstream's tool list has `pages`. A list that fails or does not
/// come within `deadline` counts as no: the bridge starts without it rather
/// than not at all.
async fn offers_pages(
    list: impl std::future::Future<Output = Result<Vec<String>>>,
    deadline: Duration,
) -> bool {
    match tokio::time::timeout(deadline, list).await {
        Ok(Ok(names)) => names.iter().any(|name| name == "pages"),
        Ok(Err(_)) | Err(_) => false,
    }
}

/// Why `linkly mcp --endpoint` fails, and what to do instead.
fn lan_bridge_unsupported(endpoint: &str) -> String {
    let base = connection::normalize_endpoint(endpoint);
    format!(
        "`linkly mcp --endpoint` is not supported: the stdio bridge only serves the Desktop on this\n\
         machine (default) or the cloud gateway (--remote).\n\n\
         To reach a Desktop on your local network, use one of:\n  \
           1. Connect your MCP client to Desktop over HTTP — copy the LAN config from\n     \
              Linkly AI Desktop > Agent usage > View MCP connection settings:\n       \
                url:     {base}/mcp\n       \
                headers: {{ \"Authorization\": \"Bearer <token>\" }}\n  \
           2. Bridge through the cloud gateway instead: linkly mcp --remote"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn lan_endpoint_fails_fast_with_guidance() {
        // Must fail before any network I/O: the address below is unroutable,
        // so a connection attempt would hang or time out instead.
        let err = run(Some("http://10.255.255.1:60606/mcp"), false, None)
            .await
            .expect_err("LAN bridging is not supported");
        let msg = err.to_string();
        assert!(msg.contains("not supported"), "{msg}");
        assert!(msg.contains("http://10.255.255.1:60606/mcp"), "{msg}");
        assert!(msg.contains("linkly mcp --remote"), "{msg}");
    }

    #[tokio::test]
    async fn pages_is_offered_when_the_upstream_lists_it() {
        let list = async { Ok(vec!["search".to_string(), "pages".to_string()]) };
        assert!(offers_pages(list, Duration::from_secs(1)).await);
    }

    #[tokio::test]
    async fn an_upstream_without_pages_or_with_a_failing_list_offers_none() {
        assert!(!offers_pages(async { Ok(vec!["search".to_string()]) }, Duration::from_secs(1)).await);
        assert!(!offers_pages(async { Err(anyhow::anyhow!("offline")) }, Duration::from_secs(1)).await);
    }

    /// A list that never answers must not keep the bridge from starting.
    #[tokio::test]
    async fn a_list_that_never_answers_counts_as_no_pages() {
        let started = std::time::Instant::now();
        let offered = offers_pages(std::future::pending(), Duration::from_millis(50)).await;
        assert!(!offered);
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
