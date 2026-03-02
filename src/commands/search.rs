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
    let mut args = serde_json::json!({ "query": query });

    if let Some(limit) = limit {
        args["limit"] = serde_json::json!(limit);
    }
    if let Some(types) = doc_types {
        args["doc_types"] = serde_json::json!(types);
    }

    match client.call_tool("search", args).await {
        Ok(content) => output::print_result(&content, json_mode),
        Err(e) => output::print_error(&e.to_string(), json_mode),
    }

    Ok(())
}
