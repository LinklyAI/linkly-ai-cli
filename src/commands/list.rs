use anyhow::Result;

use crate::client::McpClient;
use crate::commands::search::normalize_tags;
use crate::connection::ConnectionInfo;
use crate::outcome::{classify, Outcome, ResultShape};
use crate::output;

/// Run `linkly list --scope <scope>`.
///
/// A thin pass-through to the `list` MCP tool. The scope value set is
/// deliberately **not** re-validated beyond what clap accepts: the desktop owns
/// it at runtime (its schema omits the enum on purpose so a newer desktop can
/// add scopes without an older client rejecting them before sending).
#[allow(clippy::too_many_arguments)]
pub async fn run(
    client: &McpClient,
    conn: &ConnectionInfo,
    scope: &str,
    tags: Option<Vec<String>>,
    limit: Option<usize>,
    offset: Option<usize>,
    sort: Option<String>,
    no_snippet: bool,
    json_mode: bool,
) -> Result<Outcome> {
    if let Some(0) = limit {
        return output::print_error("--limit must be at least 1", json_mode);
    }

    let mut args = serde_json::json!({ "scope": scope });

    if let Some(tags) = tags {
        let tags = normalize_tags(tags);
        if tags.is_empty() {
            return output::print_error(
                "--tags was given but contains no usable tag. Pass at least one tag, e.g. --tags project",
                json_mode,
            );
        }
        args["tags"] = serde_json::json!(tags);
    }
    if let Some(limit) = limit {
        args["limit"] = serde_json::json!(limit);
    }
    if let Some(offset) = offset {
        args["offset"] = serde_json::json!(offset);
    }
    if let Some(sort) = sort {
        args["sort"] = serde_json::json!(sort);
    }
    // Only send `snippet` when the user opted out. Omitting it lets the server
    // apply the per-scope default (true for notes), which is what we want.
    if no_snippet {
        args["snippet"] = serde_json::json!(false);
    }
    if json_mode {
        args["output_format"] = serde_json::json!("json");
    }

    match client.call_tool("list", args, conn).await {
        Ok(content) => {
            let outcome = classify(&content, ResultShape::List, json_mode);
            output::print_result(&content, json_mode);
            Ok(outcome)
        }
        Err(e) => output::print_tool_error(&e, json_mode),
    }
}
