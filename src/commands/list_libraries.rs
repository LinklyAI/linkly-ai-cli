use anyhow::Result;

use crate::client::McpClient;
use crate::output;

pub async fn run(client: &McpClient, json_mode: bool) -> Result<()> {
    match client
        .call_tool("list_libraries", serde_json::json!({}))
        .await
    {
        Ok(content) => output::print_result(&content, json_mode),
        Err(e) => output::print_error(&e.to_string(), json_mode),
    }

    Ok(())
}
