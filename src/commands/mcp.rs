use std::time::Duration;

use anyhow::{bail, Result};
use rmcp::ServiceExt;

use crate::bridge::handler::StdioBridgeHandler;
use crate::bridge::upstream::{Session, Upstream};
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
    let client = client.map(str::to_string);
    let connect = move || open_session(remote, client.clone());

    // Serve even when the upstream cannot be reached: the client still gets
    // the tool list, and each tool call tries again and says what is wrong.
    let (session, pages) = match connect_and_probe(&connect).await {
        Ok((session, pages)) => (Some(session), pages),
        Err(err) => {
            eprintln!("linkly mcp: not connected yet, will retry on the next tool call.\n{err:#}");
            (None, false)
        }
    };
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

/// Connect, and learn whether to advertise `pages`: only when the upstream
/// does — it follows the account's Linkly AI Pages switch there, and an older
/// Desktop has none.
async fn connect_and_probe<F, Fut>(connect: &F) -> Result<(Session, bool)>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<Session>>,
{
    let Session { client, conn } = connect().await?;
    let (client, pages) = probe_pages(client, PAGES_PROBE_TIMEOUT, || {
        McpClient::connect_passthrough(&conn)
    })
    .await?;
    Ok((Session { client, conn }, pages))
}

/// How long the bridge waits, before it starts, to learn whether the upstream
/// offers `pages`: the client that started the bridge is waiting too.
const PAGES_PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// The client to serve through, and whether the upstream's tool list has
/// `pages`. A list that fails or does not come within `deadline` counts as no,
/// and leaves its session possibly stuck: the transport sends one request at
/// a time and waits for each answer, so every later call would queue behind
/// it. The bridge then serves through a new session from `reconnect`.
pub(crate) async fn probe_pages<F, Fut>(
    client: McpClient,
    deadline: Duration,
    reconnect: F,
) -> Result<(McpClient, bool)>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<McpClient>>,
{
    let listed = tokio::time::timeout(deadline, client.list_tools()).await;
    match listed {
        Ok(Ok(names)) => Ok((client, names.iter().any(|name| name == "pages"))),
        Ok(Err(_)) | Err(_) => {
            drop(client);
            Ok((reconnect().await?, false))
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
}
