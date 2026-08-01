use anyhow::Result;

use crate::client::McpClient;
use crate::connection::ConnectionInfo;
use crate::output;

pub async fn run(
    client: &McpClient,
    conn: &ConnectionInfo,
    ids: &[String],
    offset: Option<usize>,
    limit: Option<usize>,
    image_text: Option<String>,
    json_mode: bool,
) -> Result<()> {
    // The `read` tool takes one document, so several IDs mean several calls.
    // Sequentially: the desktop is a single local process and the point here is
    // pipeability, not throughput.
    let multi = ids.len() > 1;
    let mut failures: Vec<String> = Vec::new();
    let mut succeeded = 0usize;

    for id in ids.iter() {
        let mut args = serde_json::json!({ "doc_id": id });
        if let Some(offset) = offset {
            args["offset"] = serde_json::json!(offset);
        }
        if let Some(limit) = limit {
            args["limit"] = serde_json::json!(limit);
        }
        if let Some(ref detail) = image_text {
            args["image_text"] = serde_json::json!(detail);
        }
        if json_mode {
            args["output_format"] = serde_json::json!("json");
        }

        match client.call_tool("read", args, conn).await {
            Ok(content) => {
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
                // One unreadable document among many shouldn't discard the rest
                // — that is the whole reason for passing a batch.
                if !multi {
                    return output::print_tool_error(&e, json_mode);
                }
                failures.push(format!("{}: {}", id, e));
            }
        }
    }

    if !failures.is_empty() {
        let summary = failures.join("\n  ");
        if succeeded == 0 {
            return output::print_error(
                &format!(
                    "Could not read any of the {} documents:\n  {}",
                    ids.len(),
                    summary
                ),
                json_mode,
            );
        }
        // Partial success: the readable documents are already on stdout, so the
        // failures go to stderr where they don't corrupt a downstream parser.
        eprintln!(
            "Warning: {} of {} documents could not be read:\n  {}",
            failures.len(),
            ids.len(),
            summary
        );
    }

    Ok(())
}
