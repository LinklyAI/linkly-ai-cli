use anyhow::Result;

use crate::client::McpClient;
use crate::output;

pub async fn run(
    client: &McpClient,
    pattern: &str,
    doc_id: &str,
    context: Option<usize>,
    before: Option<usize>,
    after: Option<usize>,
    case_insensitive: bool,
    mode: Option<String>,
    limit: Option<usize>,
    offset: Option<usize>,
    fuzzy_whitespace: Option<bool>,
    json_mode: bool,
) -> Result<()> {
    let mut args = serde_json::json!({
        "pattern": pattern,
        "doc_id": doc_id,
    });

    if let Some(c) = context {
        args["context"] = serde_json::json!(c);
    }
    if let Some(b) = before {
        args["before"] = serde_json::json!(b);
    }
    if let Some(a) = after {
        args["after"] = serde_json::json!(a);
    }
    if case_insensitive {
        args["case_insensitive"] = serde_json::json!(true);
    }
    if let Some(m) = mode {
        args["output_mode"] = serde_json::json!(m);
    }
    if let Some(l) = limit {
        args["limit"] = serde_json::json!(l);
    }
    if let Some(o) = offset {
        args["offset"] = serde_json::json!(o);
    }
    if let Some(fw) = fuzzy_whitespace {
        args["fuzzy_whitespace"] = serde_json::json!(fw);
    }
    if json_mode {
        args["output_format"] = serde_json::json!("json");
    }

    match client.call_tool("grep", args).await {
        Ok(content) => output::print_result(&content, json_mode),
        Err(e) => output::print_error(&e.to_string(), json_mode),
    }

    Ok(())
}
