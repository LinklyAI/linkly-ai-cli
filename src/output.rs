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

/// Print an error message.
pub fn print_error(msg: &str, json_mode: bool) {
    if json_mode {
        let envelope = serde_json::json!({
            "status": "error",
            "message": msg,
        });
        println!("{}", envelope);
    } else {
        eprintln!("Error: {}", msg);
    }
}
