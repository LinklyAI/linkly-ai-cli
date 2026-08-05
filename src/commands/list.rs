use anyhow::Result;

use crate::client::McpClient;
use crate::commands::search::{normalize_tags, validate_doc_types};
use crate::connection::ConnectionInfo;
use crate::outcome::{classify, Outcome, ResultShape};
use crate::output;

/// Run `linkly list --scope <scope>`.
///
/// A thin pass-through to the `list` MCP tool. The scope value set is
/// deliberately **not** re-validated beyond what clap accepts: the desktop owns
/// it at runtime (its schema omits the enum on purpose so a newer desktop can
/// add scopes without an older client rejecting them before sending). The same
/// goes for the per-scope parameter matrix (which flag is valid with which
/// scope) — the desktop rejects invalid combinations with a message naming the
/// offending parameter, and re-encoding that matrix here would just let the
/// two drift apart.
#[allow(clippy::too_many_arguments)]
pub async fn run(
    client: &McpClient,
    conn: &ConnectionInfo,
    scope: &str,
    library: Option<String>,
    path: Option<String>,
    doc_types: Option<Vec<String>>,
    modified_after: Option<String>,
    modified_before: Option<String>,
    tags: Option<Vec<String>>,
    limit: Option<usize>,
    offset: Option<usize>,
    sort: Option<String>,
    snippet: bool,
    no_snippet: bool,
    json_mode: bool,
) -> Result<Outcome> {
    if let Some(0) = limit {
        return output::print_error("--limit must be at least 1", json_mode);
    }

    // Same whitelist gate as `search --type`: catches typos before a pointless
    // round-trip.
    let doc_types = match validate_doc_types(doc_types) {
        Ok(types) => types,
        Err(msg) => return output::print_error(&msg, json_mode),
    };

    let mut args = serde_json::json!({ "scope": scope });

    if let Some(library) = library {
        args["library"] = serde_json::json!(library);
    }
    if let Some(path) = path {
        args["path"] = serde_json::json!(path);
    }
    if let Some(types) = doc_types {
        args["doc_types"] = serde_json::json!(types);
    }
    if let Some(after) = modified_after {
        args["modified_after"] = serde_json::json!(after);
    }
    if let Some(before) = modified_before {
        args["modified_before"] = serde_json::json!(before);
    }
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
    // Tri-state: --snippet forces on (folder/library default to off),
    // --no-snippet forces off, and neither omits the field so the server's
    // per-scope default applies (notes on, folder/library off). clap rejects
    // passing both.
    if snippet {
        args["snippet"] = serde_json::json!(true);
    } else if no_snippet {
        args["snippet"] = serde_json::json!(false);
    }
    // `list` is the one tool whose server-side default is JSON, not markdown
    // (a notes item is a CAS handle, which has no compact line rendering). Every
    // other command can omit `output_format` and get markdown; omitting it here
    // would print raw JSON to a terminal and, worse, leave the outcome
    // classifier parsing JSON with markdown rules. So always be explicit.
    args["output_format"] = serde_json::json!(if json_mode { "json" } else { "markdown" });

    match client.call_tool("list", args, conn).await {
        Ok(content) => {
            let outcome = classify(&content, ResultShape::List, json_mode);
            output::print_result(&content, json_mode);
            Ok(outcome)
        }
        Err(e) => output::print_tool_error(&e, json_mode),
    }
}
