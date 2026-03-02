use anyhow::Result;

use crate::client::McpClient;
use crate::output;

pub async fn run(
    client: &McpClient,
    id: &str,
    offset: Option<usize>,
    limit: Option<usize>,
    json_mode: bool,
) -> Result<()> {
    let mut args = serde_json::json!({ "doc_id": id });

    if let Some(offset) = offset {
        args["offset"] = serde_json::json!(offset);
    }
    if let Some(limit) = limit {
        args["limit"] = serde_json::json!(limit);
    }

    match client.call_tool("read", args).await {
        Ok(content) => output::print_result(&content, json_mode),
        Err(e) => output::print_error(&e.to_string(), json_mode),
    }

    Ok(())
}
