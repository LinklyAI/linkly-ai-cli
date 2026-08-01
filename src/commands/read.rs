use anyhow::Result;

use crate::client::McpClient;
use crate::connection::ConnectionInfo;
use crate::output;

pub async fn run(
    client: &McpClient,
    conn: &ConnectionInfo,
    id: &str,
    offset: Option<usize>,
    limit: Option<usize>,
    image_text: Option<String>,
    json_mode: bool,
) -> Result<()> {
    let mut args = serde_json::json!({ "doc_id": id });

    if let Some(offset) = offset {
        args["offset"] = serde_json::json!(offset);
    }
    if let Some(limit) = limit {
        args["limit"] = serde_json::json!(limit);
    }
    if let Some(detail) = image_text {
        args["image_text"] = serde_json::json!(detail);
    }
    if json_mode {
        args["output_format"] = serde_json::json!("json");
    }

    match client.call_tool("read", args, conn).await {
        Ok(content) => output::print_result(&content, json_mode),
        Err(e) => return output::print_tool_error(&e, json_mode),
    }

    Ok(())
}
