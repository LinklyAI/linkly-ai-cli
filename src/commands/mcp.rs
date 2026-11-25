use std::time::Duration;

use anyhow::{bail, Result};
use rmcp::ServiceExt;

use crate::bridge::handler::StdioBridgeHandler;
use crate::bridge::upstream::{Session, Upstream};
use crate::client::McpClient;
use crate::connection;
use crate::pages_gateway::PagesGateway;

/// Run the MCP stdio bridge.
///
/// This creates a stdio MCP server that Claude Desktop (or any MCP client)
/// can connect to. All tool calls are transparently forwarded upstream, except
/// `pages`, which goes to the cloud gateway in either mode.
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
    let client = client.map(str::to_string);
    let pages = offered_pages(
        PagesGateway::from_credentials(remote, client.as_deref()),
        PAGES_PROBE_TIMEOUT,
    );
    let connect = move || open_session(remote, client.clone());

    // Serve even when the upstream cannot be reached: the client still gets
    // the tool list, and each tool call tries again and says what is wrong.
    let (session, pages) = tokio::join!(connect(), pages);
    let session = session
        .inspect_err(|err| {
            eprintln!("linkly mcp: not connected yet, will retry on the next tool call.\n{err:#}");
        })
        .ok();
    let upstream = Upstream::new(remote, session, Box::new(move || Box::pin(connect())));

    // Create the bridge handler and serve over stdio
    let handler = StdioBridgeHandler::with_pages(upstream, pages);
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

/// Resolve the connection and open a session over it.
///
/// The bridge is a transparent passthrough — skip the version gate so an old
/// Desktop's "tool not found" surfaces verbatim to whatever upstream client
/// (e.g. Claude Desktop) is using us, instead of being intercepted on the way
/// in.
async fn open_session(remote: bool, client: Option<String>) -> Result<Session> {
    let conn = connection::resolve(None, None, false, remote, client.as_deref())?;
    let client = McpClient::connect_passthrough(&conn).await?;
    Ok(Session { client, conn })
}

/// How long the bridge waits, before it starts, to learn whether the account
/// has `pages`: the client that started the bridge is waiting too.
const PAGES_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// The gateway to offer `pages` through: only when it lists the tool for the
/// account. Without a key, or when it cannot be asked, the bridge serves its
/// other tools and says on stderr what is missing. Read once: another account
/// signed in later takes a restart of the bridge.
async fn offered_pages(gateway: Result<PagesGateway>, timeout: Duration) -> Option<PagesGateway> {
    let listed = match &gateway {
        Ok(gateway) => gateway.lists_pages(timeout).await,
        Err(_) => Ok(true),
    };
    match (gateway, listed) {
        (Ok(gateway), Ok(true)) => Some(gateway),
        (Ok(_), Ok(false)) => None,
        (Err(err), _) | (Ok(_), Err(err)) => {
            eprintln!("linkly mcp: pages is not offered. {err:#} Then restart the bridge.");
            None
        }
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
    async fn pages_is_offered_only_when_the_gateway_lists_it() {
        use crate::pages_gateway::tests::fake_gateway;
        let list = |names: serde_json::Value| {
            (
                "200 OK",
                serde_json::json!({"jsonrpc": "2.0", "id": 1, "result": {"tools": names}}),
            )
        };
        let (url, _) = fake_gateway(vec![
            list(serde_json::json!([{"name": "pages"}])),
            list(serde_json::json!([{"name": "search"}])),
            ("401 Unauthorized", serde_json::json!({})),
        ])
        .await;
        let timeout = Duration::from_secs(5);
        let mut offered = Vec::new();
        for _ in 0..3 {
            let gateway = PagesGateway::at(url.clone(), false, None);
            offered.push(offered_pages(Ok(gateway), timeout).await.is_some());
        }
        assert_eq!(offered, [true, false, false]);
        assert!(offered_pages(Err(anyhow::anyhow!("no key")), timeout)
            .await
            .is_none());

        // A gateway that never answers does not keep the bridge from starting.
        let silent = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let url = format!("http://{}/mcp", silent.local_addr().expect("address"));
        let gateway = PagesGateway::at(url, false, None);
        assert!(offered_pages(Ok(gateway), Duration::from_millis(200))
            .await
            .is_none());
    }
}
