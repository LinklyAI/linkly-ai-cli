use anyhow::Result;

use crate::client::McpClient;
use crate::output;

pub async fn run(client: &McpClient, ids: &[String], json_mode: bool) -> Result<()> {
    let args = serde_json::json!({ "doc_ids": ids });

    match client.call_tool("outline", args).await {
        Ok(content) => output::print_result(&content, json_mode),
        Err(e) => output::print_error(&e.to_string(), json_mode),
    }

    Ok(())
}
