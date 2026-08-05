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

/// Upper bound on a piped note body, mirroring the desktop's own
/// `MAX_CONTENT_BYTES` (`src-tauri/src/notes/service.rs`). Without a cap,
/// `read_to_string` grows unboundedly — and this command exists precisely to be
/// fed from a pipe, so an over-long or endless producer would take the process
/// down instead of failing with a usable message. Reading one byte past the
/// limit is enough to tell "at the limit" from "over" it.
const MAX_CONTENT_BYTES: usize = 10 * 1024 * 1024;

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
    app_name: Option<String>,
    json_mode: bool,
) -> Result<()> {
    // Every argument-only check runs BEFORE `--content -` consumes stdin: a
    // flag error discovered after the read would have already drained the
    // pipe, destroying input the producer may not be able to regenerate.

    // `edit` needs both together. Checked here (not just server-side) so
    // the message can name the missing flags in CLI spelling.
    let missing = missing_edit_fields(mode, note_id.as_deref(), base_version.as_deref());
    if !missing.is_empty() {
        return output::print_error(
            &format!(
                "--mode edit requires {} (get both from `linkly list --scope notes`).",
                missing.join(", ")
            ),
            json_mode,
        );
    }

    let tags = match validate_tags(tags) {
        Ok(tags) => tags,
        Err(msg) => return output::print_error(&msg, json_mode),
    };
    let app_name = match validate_app_name(app_name) {
        Ok(app_name) => app_name,
        Err(msg) => return output::print_error(&msg, json_mode),
    };

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
        args["tags"] = serde_json::json!(tags);
    }
    if let Some(app_name) = app_name {
        args["app_name"] = serde_json::json!(app_name);
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
/// Both are required together because an edit is a compare-and-swap:
/// `note_id` says which note and `base_version` says which revision the
/// caller read. `--tags` is NOT required: the note body's inline #tokens are
/// the source of truth for tags, and the parameter can only add to them.
fn missing_edit_fields(
    mode: &str,
    note_id: Option<&str>,
    base_version: Option<&str>,
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
    missing
}

/// Normalize `--tags`, rejecting a provided-but-empty set instead of silently
/// dropping it. `--tags ""` used to mean "clear all tags" under the old
/// full-replacement model; under the additive model (Desktop ≥ 0.10.1) that
/// intent cannot be honored at all — an error that says so beats writing the
/// note with half the caller's intent discarded. Same rule as `search`/`list`:
/// an explicit `--tags` with no usable value is a caller mistake.
fn validate_tags(tags: Option<Vec<String>>) -> Result<Option<Vec<String>>, String> {
    match tags {
        None => Ok(None),
        Some(tags) => {
            let tags = normalize_tags(tags);
            if tags.is_empty() {
                Err(
                    "--tags was given but contains no usable tag. --tags can only add tags — \
                     to remove a tag, delete its #token from the note content."
                        .to_string(),
                )
            } else {
                Ok(Some(tags))
            }
        }
    }
}

/// Trim `--app-name` and reject an all-blank value; real validation (length
/// cap, control chars, reserved names) is the desktop's (#183,
/// tools/note_save.rs).
fn validate_app_name(app_name: Option<String>) -> Result<Option<String>, String> {
    match app_name {
        None => Ok(None),
        Some(name) => {
            let name = name.trim();
            if name.is_empty() {
                Err(
                    "--app-name cannot be blank. Pass the hosting application's display name, \
                     or omit the flag."
                        .to_string(),
                )
            } else {
                Ok(Some(name.to_string()))
            }
        }
    }
}

/// Resolve `--content`, reading stdin when it is the `-` sentinel.
fn read_content(content: &str) -> Result<String> {
    if content != STDIN_SENTINEL {
        return Ok(content.to_string());
    }
    read_capped(std::io::stdin().lock())
}

/// Read a note body from `reader`, refusing anything past the desktop's limit
/// rather than buffering it all first.
fn read_capped(reader: impl Read) -> Result<String> {
    let mut buf = Vec::new();
    reader
        .take(MAX_CONTENT_BYTES as u64 + 1)
        .read_to_end(&mut buf)
        .map_err(|e| anyhow::anyhow!("Failed to read note content from stdin: {}", e))?;
    if buf.len() > MAX_CONTENT_BYTES {
        anyhow::bail!(
            "Note content exceeds the {} MB limit. Notes are short cards — write the long-form content to a file and index it instead.",
            MAX_CONTENT_BYTES / (1024 * 1024)
        );
    }
    String::from_utf8(buf)
        .map_err(|_| anyhow::anyhow!("Note content from stdin is not valid UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_content_passes_through_unchanged() {
        assert_eq!(read_content("hello").unwrap(), "hello");
    }

    #[test]
    fn piped_content_under_the_cap_is_read_whole() {
        let body = "a".repeat(1024);
        assert_eq!(read_capped(body.as_bytes()).unwrap(), body);
    }

    #[test]
    fn content_exactly_at_the_cap_is_accepted() {
        let body = vec![b'a'; MAX_CONTENT_BYTES];
        assert_eq!(read_capped(&body[..]).unwrap().len(), MAX_CONTENT_BYTES);
    }

    #[test]
    fn content_past_the_cap_is_refused_instead_of_buffered() {
        let body = vec![b'a'; MAX_CONTENT_BYTES + 1];
        let err = read_capped(&body[..]).unwrap_err().to_string();
        assert!(err.contains("exceeds"), "unexpected message: {err}");
        assert!(
            err.contains("10 MB"),
            "limit should be stated in the message: {err}"
        );
    }

    #[test]
    fn invalid_utf8_on_stdin_is_a_clean_error_not_a_panic() {
        let err = read_capped(&[0xff, 0xfe][..]).unwrap_err().to_string();
        assert!(err.contains("UTF-8"), "unexpected message: {err}");
    }

    #[test]
    fn create_never_requires_the_edit_only_flags() {
        assert!(missing_edit_fields("create", None, None).is_empty());
    }

    #[test]
    fn edit_requires_note_id_and_base_version_together() {
        assert_eq!(
            missing_edit_fields("edit", None, None),
            vec!["--note-id", "--base-version"]
        );
    }

    #[test]
    fn edit_reports_only_what_is_actually_missing() {
        assert_eq!(
            missing_edit_fields("edit", Some("uuid"), None),
            vec!["--base-version"]
        );
    }

    #[test]
    fn an_edit_without_tags_passes_validation() {
        // Since Desktop 0.10.1 the note body's inline #tokens are the source
        // of truth for tags and the `tags` parameter can only add — an edit
        // that doesn't touch tags legitimately omits the flag entirely.
        assert!(missing_edit_fields("edit", Some("uuid"), Some("sha")).is_empty());
    }

    #[test]
    fn a_dash_inside_content_is_not_the_stdin_sentinel() {
        // Only a bare "-" means stdin; text that merely contains one must not
        // be diverted to a stdin read (which would block on a TTY).
        assert_eq!(read_content("a - b").unwrap(), "a - b");
        assert_eq!(read_content("--").unwrap(), "--");
    }

    #[test]
    fn an_explicitly_empty_tag_set_is_an_error_not_a_silent_no_op() {
        // `--tags ""` used to clear tags under the full-replacement model.
        // Under the additive model that intent can't be honored — the note
        // must NOT be written with the flag silently dropped (exit 0 with no
        // machine-readable signal). The error also names the actual way to
        // remove a tag.
        let err = validate_tags(Some(vec!["".to_string()])).unwrap_err();
        assert!(err.contains("#token"), "unexpected message: {err}");
        assert!(err.contains("--tags"), "unexpected message: {err}");
    }

    #[test]
    fn tags_are_normalized_and_empty_entries_dropped() {
        assert_eq!(
            validate_tags(Some(vec!["a".into(), "".into(), "b".into()])).unwrap(),
            Some(vec!["a".to_string(), "b".to_string()])
        );
        assert_eq!(validate_tags(None).unwrap(), None);
    }

    #[test]
    fn blank_app_name_is_rejected_and_valid_one_is_trimmed() {
        assert!(validate_app_name(Some("   ".into())).is_err());
        assert_eq!(
            validate_app_name(Some("  Cursor ".into())).unwrap(),
            Some("Cursor".to_string())
        );
        assert_eq!(validate_app_name(None).unwrap(), None);
    }
}
