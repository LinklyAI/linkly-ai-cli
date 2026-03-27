use anyhow::Result;

use crate::client::McpClient;
use crate::connection::ConnectionInfo;
use crate::constants::VALID_DOC_TYPES;
use crate::output;

#[allow(clippy::too_many_arguments)]
pub async fn run(
    client: &McpClient,
    conn: &ConnectionInfo,
    query: &str,
    limit: Option<usize>,
    doc_types: Option<Vec<String>>,
    library: Option<String>,
    path_glob: Option<String>,
    json_mode: bool,
) -> Result<()> {
    if query.trim().is_empty() {
        output::print_error("Search query cannot be empty", json_mode);
        return Ok(());
    }

    if let Some(0) = limit {
        output::print_error("--limit must be at least 1", json_mode);
        return Ok(());
    }

    // Normalize doc types to lowercase and validate against whitelist
    let doc_types = if let Some(types) = doc_types {
        let normalized: Vec<String> = types.iter().map(|t| t.to_lowercase()).collect();
        let invalid: Vec<&str> = normalized
            .iter()
            .filter(|t| !VALID_DOC_TYPES.contains(&t.as_str()))
            .map(|t| t.as_str())
            .collect();
        if !invalid.is_empty() {
            output::print_error(
                &format!(
                    "Unknown document type(s): {}. Supported: {}",
                    invalid.join(", "),
                    VALID_DOC_TYPES.join(", ")
                ),
                json_mode,
            );
            return Ok(());
        }
        Some(normalized)
    } else {
        None
    };

    let mut args = serde_json::json!({ "query": query });

    if let Some(limit) = limit {
        args["limit"] = serde_json::json!(limit);
    }
    if let Some(types) = doc_types {
        args["doc_types"] = serde_json::json!(types);
    }
    if let Some(lib) = library {
        args["library"] = serde_json::json!(lib);
    }
    if let Some(glob) = path_glob {
        args["path_glob"] = serde_json::json!(glob);
    }
    if json_mode {
        args["output_format"] = serde_json::json!("json");
    }

    match client.call_tool("search", args, conn).await {
        Ok(content) => output::print_result(&content, json_mode),
        Err(e) => output::print_error(&e.to_string(), json_mode),
    }

    Ok(())
}
