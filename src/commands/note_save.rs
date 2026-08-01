use std::io::Read;

use anyhow::Result;

use crate::client::McpClient;
use crate::commands::search::normalize_tags;
use crate::connection::ConnectionInfo;
use crate::output;

/// Sentinel for "read the note body from stdin", following the usual CLI
/// convention. Long note bodies are awkward to pass as a shell argument, and
/// agents composing a note from prior output already have it on a pipe.
const STDIN_SENTINEL: &str = "-";

/// Run `linkly note-save --mode create|edit`.
///
/// The desktop owns all YAML front matter and all validation (markdown
/// whitelist, tag policy, CAS on `base_version`). This command only checks the
/// mode/field combinations that would otherwise cost a pointless round-trip,
/// and leaves content rules to the server so the CLI can't drift from them.
// Matches the flat shape every other command in this module uses; bundling
// these into a struct would make this one command's call site differ from the
// rest for no gain.
#[allow(clippy::too_many_arguments)]
pub async fn run(
    client: &McpClient,
    conn: &ConnectionInfo,
    mode: &str,
    content: &str,
    note_id: Option<String>,
    base_version: Option<String>,
    tags: Option<Vec<String>>,
    json_mode: bool,
) -> Result<()> {
    // `edit` needs all three together. Checked here (not just server-side) so
    // the message can name the missing flags in CLI spelling.
    let missing = missing_edit_fields(
        mode,
        note_id.as_deref(),
        base_version.as_deref(),
        tags.as_deref(),
    );
    if !missing.is_empty() {
        return output::print_error(
            &format!(
                "--mode edit requires {} (get --note-id and --base-version from `linkly list --scope notes`).\n\
                 Note: --tags is the FULL replacement set — pass back every tag the note should keep.",
                missing.join(", ")
            ),
            json_mode,
        );
    }

    let body = match read_content(content) {
        Ok(body) => body,
        Err(e) => return output::print_error(&e.to_string(), json_mode),
    };
    if body.trim().is_empty() {
        return output::print_error("Note content cannot be empty", json_mode);
    }

    let mut args = serde_json::json!({
        "mode": mode,
        "content": body,
    });

    if let Some(id) = note_id {
        args["note_id"] = serde_json::json!(id);
    }
    if let Some(version) = base_version {
        args["base_version"] = serde_json::json!(version);
    }
    if let Some(tags) = tags {
        // Distinct from search/list: an explicit empty tag set is meaningful on
        // edit (it clears every tag), so an emptied list is forwarded as `[]`
        // rather than rejected. `--tags ""` is how you strip a note's tags.
        args["tags"] = serde_json::json!(normalize_tags(tags));
    }

    match client.call_tool("note_save", args, conn).await {
        Ok(content) => output::print_result(&content, json_mode),
        Err(e) => return output::print_tool_error(&e, json_mode),
    }

    Ok(())
}

/// Which of the edit-only flags are missing. Empty for `create`, and empty for
/// a complete `edit`.
///
/// All three are required together because an edit is a compare-and-swap:
/// `note_id` says which note, `base_version` says which revision the caller
/// read, and `tags` is a full replacement set that would otherwise be silently
/// cleared.
fn missing_edit_fields(
    mode: &str,
    note_id: Option<&str>,
    base_version: Option<&str>,
    tags: Option<&[String]>,
) -> Vec<&'static str> {
    if mode != "edit" {
        return Vec::new();
    }
    let mut missing = Vec::new();
    if note_id.is_none() {
        missing.push("--note-id");
    }
    if base_version.is_none() {
        missing.push("--base-version");
    }
    if tags.is_none() {
        missing.push("--tags");
    }
    missing
}

/// Resolve `--content`, reading stdin when it is the `-` sentinel.
fn read_content(content: &str) -> Result<String> {
    if content != STDIN_SENTINEL {
        return Ok(content.to_string());
    }
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .map_err(|e| anyhow::anyhow!("Failed to read note content from stdin: {}", e))?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_content_passes_through_unchanged() {
        assert_eq!(read_content("hello").unwrap(), "hello");
    }

    #[test]
    fn create_never_requires_the_edit_only_flags() {
        assert!(missing_edit_fields("create", None, None, None).is_empty());
    }

    #[test]
    fn edit_requires_all_three_flags_together() {
        assert_eq!(
            missing_edit_fields("edit", None, None, None),
            vec!["--note-id", "--base-version", "--tags"]
        );
    }

    #[test]
    fn edit_reports_only_what_is_actually_missing() {
        let tags = vec!["work".to_string()];
        assert_eq!(
            missing_edit_fields("edit", Some("uuid"), None, Some(&tags)),
            vec!["--base-version"]
        );
    }

    #[test]
    fn a_complete_edit_passes_validation() {
        let tags = vec!["work".to_string()];
        assert!(missing_edit_fields("edit", Some("uuid"), Some("sha"), Some(&tags)).is_empty());
    }

    #[test]
    fn an_explicitly_empty_tag_set_still_counts_as_provided() {
        // `--tags ""` is how a caller strips every tag from a note; it must not
        // be reported as a missing flag.
        let empty: Vec<String> = Vec::new();
        assert!(missing_edit_fields("edit", Some("uuid"), Some("sha"), Some(&empty)).is_empty());
    }

    #[test]
    fn a_dash_inside_content_is_not_the_stdin_sentinel() {
        // Only a bare "-" means stdin; text that merely contains one must not
        // be diverted to a stdin read (which would block on a TTY).
        assert_eq!(read_content("a - b").unwrap(), "a - b");
        assert_eq!(read_content("--").unwrap(), "--");
    }
}
