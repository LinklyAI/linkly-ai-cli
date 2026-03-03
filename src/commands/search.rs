use anyhow::Result;

use crate::client::McpClient;
use crate::output;

pub async fn run(
    client: &McpClient,
    query: &str,
    limit: Option<usize>,
    doc_types: Option<Vec<String>>,
    json_mode: bool,
) -> Result<()> {
    if let Some(0) = limit {
        output::print_error("--limit must be at least 1", json_mode);
        return Ok(());
    }

    let mut args = serde_json::json!({ "query": query });

    if let Some(limit) = limit {
        args["limit"] = serde_json::json!(limit);
    }
    if let Some(types) = doc_types {
        args["doc_types"] = serde_json::json!(types);
    }
    if json_mode {
        args["output_format"] = serde_json::json!("json");
    }

    match client.call_tool("search", args).await {
        Ok(content) => output::print_result(&content, json_mode),
        Err(e) => output::print_error(&e.to_string(), json_mode),
    }

    Ok(())
}
