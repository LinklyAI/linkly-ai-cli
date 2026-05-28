use anyhow::Result;

use crate::client::McpClient;
use crate::connection::ConnectionInfo;
use crate::output;

pub async fn run(
    client: &McpClient,
    conn: &ConnectionInfo,
    ids: &[String],
    expand: Option<Vec<String>>,
    json_mode: bool,
) -> Result<()> {
    let mut args = serde_json::json!({ "doc_ids": ids });

    if let Some(expand) = expand {
        args["expand"] = serde_json::json!(expand);
    }
    if json_mode {
        args["output_format"] = serde_json::json!("json");
    }

    match client.call_tool("outline", args, conn).await {
        Ok(content) => output::print_result(&content, json_mode),
        Err(e) => return output::print_tool_error(&e, json_mode),
    }

    Ok(())
}
