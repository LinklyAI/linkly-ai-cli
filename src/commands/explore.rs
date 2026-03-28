use anyhow::Result;

use crate::client::McpClient;
use crate::connection::ConnectionInfo;
use crate::output;

pub async fn run(
    client: &McpClient,
    conn: &ConnectionInfo,
    library: Option<String>,
    json_mode: bool,
) -> Result<()> {
    let mut args = serde_json::json!({});
    if let Some(lib) = library {
        args["library"] = serde_json::json!(lib);
    }

    match client.call_tool("explore", args, conn).await {
        Ok(content) => output::print_result(&content, json_mode),
        Err(e) => output::print_error(&e.to_string(), json_mode),
    }

    Ok(())
}
