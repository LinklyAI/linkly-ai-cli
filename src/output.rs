/// Print a successful result.
///
/// In normal mode, the MCP server returns well-formatted Markdown — print as-is.
/// In JSON mode, try to parse structured JSON from server and merge status field;
/// fall back to wrapping as content string for backward compatibility.
pub fn print_result(content: &str, json_mode: bool) {
    if json_mode {
        if let Ok(mut data) = serde_json::from_str::<serde_json::Value>(content) {
            if let Some(obj) = data.as_object_mut() {
                obj.insert("status".to_string(), serde_json::json!("success"));
            }
            println!("{}", data);
        } else {
            // Fallback: wrap as content string (backward compatible with older MCP server)
            let envelope = serde_json::json!({
                "status": "success",
                "content": content,
            });
            println!("{}", envelope);
        }
    } else {
        println!("{}", content);
    }
}

/// Print an error message and return an `Err` so callers can propagate
/// it via `return print_error(...)` or `?`.
///
/// The returned error carries an empty message because the user-visible
/// text has already been displayed by this function — `main.rs` checks
/// for the empty string and skips its own `Error: …` re-print to avoid
/// the duplicate. Without this propagation, a CLI command that hit a
/// validation failure would print the error and then exit 0, breaking
/// shell pipelines like `linkly search "" && deploy` and CI scripts
/// that key off `$?`.
pub fn print_error(msg: &str, json_mode: bool) -> anyhow::Result<()> {
    if json_mode {
        let envelope = serde_json::json!({
            "status": "error",
            "message": msg,
        });
        println!("{}", envelope);
    } else {
        eprintln!("Error: {}", msg);
    }
    Err(anyhow::Error::msg(""))
}
