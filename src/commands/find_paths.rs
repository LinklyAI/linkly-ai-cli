use anyhow::Result;

use crate::client::McpClient;
use crate::connection::ConnectionInfo;
use crate::output;

pub async fn run(
    client: &McpClient,
    conn: &ConnectionInfo,
    patterns: Vec<String>,
    library: Option<String>,
    limit: Option<u32>,
    json_mode: bool,
) -> Result<()> {
    if patterns.is_empty() {
        return output::print_error("--patterns must contain at least one keyword", json_mode);
    }
    if let Some(0) = limit {
        return output::print_error("--limit must be at least 1", json_mode);
    }

    let mut args = serde_json::json!({ "patterns": patterns });
    if let Some(lib) = library {
        args["library"] = serde_json::json!(lib);
    }
    if let Some(n) = limit {
        args["limit"] = serde_json::json!(n);
    }
    if json_mode {
        args["output_format"] = serde_json::json!("json");
    }

    match client.call_tool("find_paths", args, conn).await {
        Ok(content) => output::print_result(&content, json_mode),
        Err(e) => return output::print_error(&e.to_string(), json_mode),
    }

    Ok(())
}
