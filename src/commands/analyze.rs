use anyhow::Result;

use crate::client::McpClient;
use crate::connection::ConnectionInfo;
use crate::output;

/// What `linkly analyze` was asked for. Everything is optional: with nothing
/// set the tool returns its summary alone.
pub struct AnalyzeArgs {
    pub sections: Vec<String>,
    pub library: Option<String>,
    pub path: Option<String>,
    pub doc_types: Vec<String>,
    pub modified_after: Option<String>,
    pub modified_before: Option<String>,
    pub limit: Option<u32>,
}

/// The tool's arguments. Only what was given is sent: the Desktop rejects
/// fields it does not know and treats an omitted one as its default. There is
/// no `output_format`: the tool always answers in JSON.
fn tool_args(input: AnalyzeArgs) -> serde_json::Value {
    let mut args = serde_json::json!({});
    if !input.sections.is_empty() {
        args["sections"] = serde_json::json!(input.sections);
    }
    if let Some(library) = input.library {
        args["library"] = serde_json::json!(library);
    }
    if let Some(path) = input.path {
        args["path"] = serde_json::json!(path);
    }
    if !input.doc_types.is_empty() {
        args["doc_types"] = serde_json::json!(input.doc_types);
    }
    if let Some(after) = input.modified_after {
        args["modified_after"] = serde_json::json!(after);
    }
    if let Some(before) = input.modified_before {
        args["modified_before"] = serde_json::json!(before);
    }
    if let Some(limit) = input.limit {
        args["limit"] = serde_json::json!(limit);
    }
    args
}

pub async fn run(
    client: &McpClient,
    conn: &ConnectionInfo,
    input: AnalyzeArgs,
    json_mode: bool,
) -> Result<()> {
    if let Some(0) = input.limit {
        return output::print_error("--limit must be at least 1", json_mode);
    }

    match client.call_tool("analyze", tool_args(input), conn).await {
        Ok(content) => output::print_result(&content, json_mode),
        Err(e) => return output::print_tool_error(&e, json_mode),
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nothing() -> AnalyzeArgs {
        AnalyzeArgs {
            sections: Vec::new(),
            library: None,
            path: None,
            doc_types: Vec::new(),
            modified_after: None,
            modified_before: None,
            limit: None,
        }
    }

    #[test]
    fn sends_only_what_was_given() {
        assert_eq!(tool_args(nothing()), serde_json::json!({}));

        let args = tool_args(AnalyzeArgs {
            sections: vec!["types".into(), "time".into()],
            path: Some("/Users/me/Documents".into()),
            doc_types: vec!["pdf".into()],
            modified_after: Some("2024-01-01".into()),
            limit: Some(20),
            ..nothing()
        });
        assert_eq!(
            args,
            serde_json::json!({
                "sections": ["types", "time"],
                "path": "/Users/me/Documents",
                "doc_types": ["pdf"],
                "modified_after": "2024-01-01",
                "limit": 20,
            })
        );
        // The tool has no such field and would reject the call.
        assert!(args.get("output_format").is_none());
    }
}
