/// Print a successful result.
///
/// In normal mode, the MCP server returns well-formatted Markdown — print as-is.
/// In JSON mode, wrap in a `{"status":"success","content":...}` envelope.
pub fn print_result(content: &str, json_mode: bool) {
    if json_mode {
        let envelope = serde_json::json!({
            "status": "success",
            "content": content,
        });
        println!("{}", envelope);
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
