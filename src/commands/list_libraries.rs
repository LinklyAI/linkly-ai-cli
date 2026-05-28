use anyhow::Result;

use crate::client::McpClient;
use crate::connection::ConnectionInfo;
use crate::output;

pub async fn run(client: &McpClient, conn: &ConnectionInfo, json_mode: bool) -> Result<()> {
    match client
        .call_tool("list_libraries", serde_json::json!({}), conn)
        .await
    {
        Ok(content) => output::print_result(&content, json_mode),
        Err(e) => return output::print_tool_error(&e, json_mode),
    }

    Ok(())
}
