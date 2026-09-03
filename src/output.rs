use crate::client::ToolError;

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
                add_skills_hint(obj);
            }
            println!("{}", data);
        } else {
            // Fallback: wrap as content string (backward compatible with older MCP server)
            let mut envelope = serde_json::json!({
                "status": "success",
                "content": content,
            });
            if let Some(obj) = envelope.as_object_mut() {
                add_skills_hint(obj);
            }
            println!("{}", envelope);
        }
    } else {
        println!("{}", content);
    }
}

/// Attach the skills notice when one is pending.
///
/// The underscore prefix marks it as metadata about the call rather than part
/// of the answer, matching `_meta` in the desktop's MCP responses. Errors do
/// not carry it: a failure envelope is read to find out what went wrong, and
/// an unrelated advisory there only dilutes the signal.
fn add_skills_hint(obj: &mut serde_json::Map<String, serde_json::Value>) {
    if let Some(hint) = crate::skills::hint() {
        obj.insert("_skills_hint".to_string(), serde_json::json!(hint));
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
pub fn print_error<T>(msg: &str, json_mode: bool) -> anyhow::Result<T> {
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

/// Print a tool-call error, returning an `Err` under the same empty-message
/// propagation contract as [`print_error`].
///
/// When the underlying error is a structured JSON-RPC error from the gateway
/// ([`ToolError::Mcp`]), JSON mode emits `code` and `data` (which carries the
/// gateway's `guidance` / `example`) as dedicated fields instead of flattening
/// everything into `message`. Any other error (timeout, transport, tool-level
/// `isError`, bad arguments) falls back to the plain-string rendering.
pub fn print_tool_error<T>(err: &anyhow::Error, json_mode: bool) -> anyhow::Result<T> {
    if let Some(ToolError {
        code,
        message,
        data,
    }) = err.downcast_ref::<ToolError>()
    {
        if json_mode {
            println!("{}", tool_error_envelope(*code, message, data.as_ref()));
        } else {
            eprintln!("Error: {}: {}", code, message);
            if let Some(data) = data {
                eprintln!("{}", data);
            }
        }
        return Err(anyhow::Error::msg(""));
    }
    print_error(&err.to_string(), json_mode)
}

/// Build the JSON error envelope for a structured tool error. `data` is included
/// only when present (absent rather than `null`).
fn tool_error_envelope(
    code: i32,
    message: &str,
    data: Option<&serde_json::Value>,
) -> serde_json::Value {
    let mut envelope = serde_json::json!({
        "status": "error",
        "code": code,
        "message": message,
    });
    if let Some(data) = data {
        envelope["data"] = data.clone();
    }
    envelope
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_includes_data_when_present() {
        let data = serde_json::json!({ "guidance": ["x"] });
        let env = tool_error_envelope(-32602, "bad params", Some(&data));
        assert_eq!(env["status"], "error");
        assert_eq!(env["code"], -32602);
        assert_eq!(env["message"], "bad params");
        assert_eq!(env["data"], data);
    }

    #[test]
    fn envelope_omits_data_when_none() {
        let env = tool_error_envelope(-32000, "offline", None);
        assert_eq!(env["code"], -32000);
        assert_eq!(env["message"], "offline");
        assert!(env.get("data").is_none());
    }

    #[test]
    fn print_tool_error_structured_returns_empty_err() {
        let err: anyhow::Error = ToolError {
            code: -32000,
            message: "x".into(),
            data: None,
        }
        .into();
        let result: anyhow::Result<()> = print_tool_error(&err, true);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().to_string(), "");
    }

    #[test]
    fn print_tool_error_plain_returns_empty_err() {
        let err = anyhow::anyhow!("plain transport error");
        let result: anyhow::Result<()> = print_tool_error(&err, true);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().to_string(), "");
    }
}

#[cfg(test)]
mod skills_hint_tests {
    /// The notice has to reach JSON consumers too — plain-text output is the
    /// primary path, but an agent that asked for `--json` would otherwise
    /// never learn its skill is stale.
    #[test]
    fn json_envelopes_carry_the_notice() {
        crate::skills::publish_hint(Some("[linkly] Skills v1.0.0 installed".to_string()));

        let mut obj = serde_json::Map::new();
        obj.insert("status".into(), serde_json::json!("success"));
        super::add_skills_hint(&mut obj);

        assert_eq!(
            obj.get("_skills_hint").and_then(|v| v.as_str()),
            Some("[linkly] Skills v1.0.0 installed")
        );
    }
}
