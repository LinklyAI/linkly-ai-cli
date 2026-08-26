use anyhow::Result;

use crate::client::McpClient;
use crate::connection::ConnectionInfo;
use crate::constants::VALID_DOC_TYPES;
use crate::outcome::{classify, Outcome, ResultShape};
use crate::output;

#[allow(clippy::too_many_arguments)]
pub async fn run(
    client: &McpClient,
    conn: &ConnectionInfo,
    query: &str,
    limit: Option<usize>,
    doc_types: Option<Vec<String>>,
    library: Option<String>,
    path_glob: Option<String>,
    modified_after: Option<String>,
    modified_before: Option<String>,
    time_sort: Option<String>,
    scope: Option<String>,
    tags: Option<Vec<String>>,
    json_mode: bool,
) -> Result<Outcome> {
    if query.trim().is_empty() {
        return output::print_error("Search query cannot be empty", json_mode);
    }

    if let Some(0) = limit {
        return output::print_error("--limit must be at least 1", json_mode);
    }

    // Normalize doc types to lowercase and validate against the whitelist.
    let doc_types = match validate_doc_types(doc_types) {
        Ok(types) => types,
        Err(msg) => return output::print_error(&msg, json_mode),
    };

    let mut args = serde_json::json!({ "query": query });

    crate::commands::set_optional_arg(&mut args, "limit", limit);
    crate::commands::set_optional_arg(&mut args, "doc_types", doc_types);
    crate::commands::set_optional_arg(&mut args, "library", library);
    crate::commands::set_optional_arg(&mut args, "path_glob", path_glob);
    crate::commands::set_optional_arg(&mut args, "modified_after", modified_after);
    crate::commands::set_optional_arg(&mut args, "modified_before", modified_before);
    crate::commands::set_optional_arg(&mut args, "time_sort", time_sort);
    crate::commands::set_optional_arg(&mut args, "scope", scope);
    if let Some(tags) = tags {
        // An explicit `--tags` with no usable value is a caller mistake, not an
        // empty filter: sending `[]` would silently widen the search back to
        // everything, which reads as "the filter worked and matched all notes".
        let tags = normalize_tags(tags);
        if tags.is_empty() {
            return output::print_error(
                "--tags was given but contains no usable tag. Pass at least one tag, e.g. --tags project",
                json_mode,
            );
        }
        args["tags"] = serde_json::json!(tags);
    }
    if json_mode {
        args["output_format"] = serde_json::json!("json");
    }

    match client.call_tool("search", args, conn).await {
        Ok(content) => {
            let outcome = classify(&content, ResultShape::Search, json_mode);
            output::print_result(&content, json_mode);
            Ok(outcome)
        }
        Err(e) => output::print_tool_error(&e, json_mode),
    }
}

/// Normalize document-type filters to lowercase and validate them against the
/// whitelist of supported types ([`VALID_DOC_TYPES`]).
///
/// Returns:
/// - `Ok(None)` when no filter was provided
/// - `Ok(Some(normalized))` when every type is valid (lowercased, order preserved)
/// - `Err(message)` listing the unknown type(s) and the supported set, ready to
///   surface to the user
pub(crate) fn validate_doc_types(
    doc_types: Option<Vec<String>>,
) -> Result<Option<Vec<String>>, String> {
    let Some(types) = doc_types else {
        return Ok(None);
    };
    let normalized: Vec<String> = types.iter().map(|t| t.to_lowercase()).collect();
    let invalid: Vec<&str> = normalized
        .iter()
        .filter(|t| !VALID_DOC_TYPES.contains(&t.as_str()))
        .map(|t| t.as_str())
        .collect();
    if !invalid.is_empty() {
        return Err(format!(
            "Unknown document type(s): {}. Supported: {}",
            invalid.join(", "),
            VALID_DOC_TYPES.join(", ")
        ));
    }
    Ok(Some(normalized))
}

/// Normalize note tags the same way the desktop does: strip a leading `#` and
/// ASCII-lowercase. Empty entries (from `--tags a,,b` or a stray comma) are
/// dropped rather than forwarded as empty strings, which the server would
/// reject.
///
/// Kept in the CLI so `--tags '#Work'` behaves identically to `--tags work`
/// without a round-trip; the server normalizes again, so this is convergent
/// rather than authoritative.
pub(crate) fn normalize_tags(tags: Vec<String>) -> Vec<String> {
    tags.iter()
        .map(|t| t.trim().trim_start_matches('#').to_ascii_lowercase())
        .filter(|t| !t.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_are_normalized_and_emptied_entries_dropped() {
        assert_eq!(
            normalize_tags(vec!["#Work".into(), " Project ".into(), "".into()]),
            vec!["work".to_string(), "project".to_string()]
        );
    }

    #[test]
    fn tags_of_only_separators_normalize_to_empty() {
        assert!(normalize_tags(vec!["".into(), "#".into(), "  ".into()]).is_empty());
    }

    #[test]
    fn audio_and_video_are_accepted() {
        assert_eq!(
            validate_doc_types(Some(vec!["audio".into(), "video".into()])),
            Ok(Some(vec!["audio".into(), "video".into()]))
        );
    }

    #[test]
    fn none_input_returns_none() {
        assert_eq!(validate_doc_types(None), Ok(None));
    }

    #[test]
    fn valid_types_are_lowercased() {
        assert_eq!(
            validate_doc_types(Some(vec!["PDF".into(), "Md".into()])),
            Ok(Some(vec!["pdf".into(), "md".into()]))
        );
    }

    #[test]
    fn pptx_is_accepted() {
        assert_eq!(
            validate_doc_types(Some(vec!["pptx".into()])),
            Ok(Some(vec!["pptx".into()]))
        );
    }

    #[test]
    fn xlsx_and_csv_are_accepted() {
        assert_eq!(
            validate_doc_types(Some(vec!["xlsx".into(), "CSV".into()])),
            Ok(Some(vec!["xlsx".into(), "csv".into()]))
        );
        // Spreadsheet containers the desktop does not scan stay rejected.
        assert!(validate_doc_types(Some(vec!["xls".into()])).is_err());
    }

    #[test]
    fn epub_is_accepted() {
        assert_eq!(
            validate_doc_types(Some(vec!["epub".into()])),
            Ok(Some(vec!["epub".into()]))
        );
    }

    #[test]
    fn mixed_valid_types_pass() {
        assert_eq!(
            validate_doc_types(Some(vec!["pdf".into(), "pptx".into(), "md".into()])),
            Ok(Some(vec!["pdf".into(), "pptx".into(), "md".into()]))
        );
    }

    #[test]
    fn unknown_type_is_rejected_and_lists_supported() {
        let err = validate_doc_types(Some(vec!["xyz".into()])).unwrap_err();
        assert!(err.contains("xyz"));
        assert!(err.contains("Supported:"));
        // Regression guard: pptx must be advertised as a supported type.
        assert!(err.contains("pptx"));
        // Regression guard: epub must be advertised as a supported type.
        assert!(err.contains("epub"));
        // Regression guard: the media types must be advertised too — they were
        // missing from the whitelist while the desktop already indexed them.
        assert!(err.contains("audio"));
        assert!(err.contains("video"));
    }

    #[test]
    fn only_invalid_types_are_listed_not_valid_ones() {
        // A valid type (pdf) alongside an invalid one must not appear in the
        // "unknown" segment (the part before ". Supported:").
        let err = validate_doc_types(Some(vec!["pdf".into(), "bogus".into()])).unwrap_err();
        let unknown_segment = err.split(". Supported:").next().unwrap();
        assert_eq!(unknown_segment, "Unknown document type(s): bogus");
    }
}
