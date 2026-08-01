use anyhow::Result;

use crate::client::McpClient;
use crate::connection::ConnectionInfo;
use crate::outcome::{classify, Outcome, ResultShape};
use crate::output;

#[allow(clippy::too_many_arguments)]
pub async fn run(
    client: &McpClient,
    conn: &ConnectionInfo,
    pattern: &str,
    doc_ids: &[String],
    context: Option<usize>,
    before: Option<usize>,
    after: Option<usize>,
    case_insensitive: bool,
    mode: Option<String>,
    limit: Option<usize>,
    offset: Option<usize>,
    fuzzy_whitespace: Option<bool>,
    json_mode: bool,
) -> Result<Outcome> {
    // `grep` takes one document per call, so a batch means a loop. Scanning a
    // set of documents for a pattern is the main reason to pipe search results
    // in, so this is the case the stdin sentinel exists for.
    let multi = doc_ids.len() > 1;
    let mut failures: Vec<String> = Vec::new();
    let mut found_any = false;
    let mut succeeded = 0usize;

    for doc_id in doc_ids.iter() {
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
        if let Some(ref m) = mode {
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

        match client.call_tool("grep", args, conn).await {
            Ok(content) => {
                if classify(&content, ResultShape::Grep, json_mode) == Outcome::Found {
                    found_any = true;
                }
                // JSON mode stays one object per line: a single ID produces
                // exactly what it always did, several produce JSON Lines —
                // streamable, and readable by `jq` without a wrapper array.
                //
                // Markdown needs no separator of its own; each response already
                // ends with the `---` of its metadata footer, and every document
                // starts with a heading.
                output::print_result(&content, json_mode);
                succeeded += 1;
            }
            Err(e) => {
                if !multi {
                    return output::print_tool_error(&e, json_mode);
                }
                failures.push(format!("{}: {}", doc_id, e));
            }
        }
    }

    if !failures.is_empty() {
        let summary = failures.join("\n  ");
        if succeeded == 0 {
            return output::print_error(
                &format!(
                    "Could not grep any of the {} documents:\n  {}",
                    doc_ids.len(),
                    summary
                ),
                json_mode,
            );
        }
        eprintln!(
            "Warning: {} of {} documents could not be searched:\n  {}",
            failures.len(),
            doc_ids.len(),
            summary
        );
    }

    // Across a batch, "found" means at least one document matched — the same
    // thing `grep pattern *.txt` reports.
    Ok(if found_any {
        Outcome::Found
    } else {
        Outcome::Empty
    })
}
