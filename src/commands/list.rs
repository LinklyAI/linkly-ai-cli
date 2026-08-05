use anyhow::Result;

use crate::client::McpClient;
use crate::commands::search::{normalize_tags, validate_doc_types};
use crate::connection::ConnectionInfo;
use crate::outcome::{classify, Outcome, ResultShape};
use crate::output;

/// Everything `linkly list` collected from the command line.
///
/// Named fields on purpose: as a flat signature this was 14 positional
/// parameters, four of them adjacent `Option<String>`s — transposing
/// `modified_after`/`modified_before` (or `library`/`path`) at the call site
/// type-checked silently and only failed at the server. Struct construction
/// with field names makes that mistake unrepresentable.
pub struct ListParams {
    pub scope: String,
    pub library: Option<String>,
    pub path: Option<String>,
    pub doc_types: Option<Vec<String>>,
    pub modified_after: Option<String>,
    pub modified_before: Option<String>,
    pub tags: Option<Vec<String>>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    pub sort: Option<String>,
    pub snippet: bool,
    pub no_snippet: bool,
}

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
pub async fn run(
    client: &McpClient,
    conn: &ConnectionInfo,
    params: ListParams,
    json_mode: bool,
) -> Result<Outcome> {
    let ListParams {
        scope,
        library,
        path,
        doc_types,
        modified_after,
        modified_before,
        tags,
        limit,
        offset,
        sort,
        snippet,
        no_snippet,
    } = params;
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

    crate::commands::set_optional_arg(&mut args, "library", library);
    crate::commands::set_optional_arg(&mut args, "path", path);
    crate::commands::set_optional_arg(&mut args, "doc_types", doc_types);
    crate::commands::set_optional_arg(&mut args, "modified_after", modified_after);
    crate::commands::set_optional_arg(&mut args, "modified_before", modified_before);
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
    crate::commands::set_optional_arg(&mut args, "limit", limit);
    crate::commands::set_optional_arg(&mut args, "offset", offset);
    crate::commands::set_optional_arg(&mut args, "sort", sort);
    // Tri-state: --snippet forces on (folder/library default to off),
    // --no-snippet forces off, and neither omits the field so the server's
    // per-scope default applies (notes on, folder/library off). clap rejects
    // passing both.
    if snippet {
        args["snippet"] = serde_json::json!(true);
    } else if no_snippet {
        args["snippet"] = serde_json::json!(false);
    }
    // `list` is the one tool whose server-side `output_format` default varies
    // BY SCOPE: notes=json (a notes item is a CAS handle, which has no compact
    // line rendering), folder/library=markdown. Always send it explicitly for
    // every scope — omitting it would make the rendering depend on which scope
    // was asked for, print raw JSON to a terminal for notes and, worse, leave
    // the outcome classifier parsing one format with the other's rules.
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
