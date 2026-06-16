use anyhow::Result;

use crate::client::McpClient;
use crate::connection::ConnectionInfo;
use crate::constants::VALID_DOC_TYPES;
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
    json_mode: bool,
) -> Result<()> {
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

    if let Some(limit) = limit {
        args["limit"] = serde_json::json!(limit);
    }
    if let Some(types) = doc_types {
        args["doc_types"] = serde_json::json!(types);
    }
    if let Some(lib) = library {
        args["library"] = serde_json::json!(lib);
    }
    if let Some(glob) = path_glob {
        args["path_glob"] = serde_json::json!(glob);
    }
    if let Some(after) = modified_after {
        args["modified_after"] = serde_json::json!(after);
    }
    if let Some(before) = modified_before {
        args["modified_before"] = serde_json::json!(before);
    }
    if let Some(sort) = time_sort {
        args["time_sort"] = serde_json::json!(sort);
    }
    if json_mode {
        args["output_format"] = serde_json::json!("json");
    }

    match client.call_tool("search", args, conn).await {
        Ok(content) => output::print_result(&content, json_mode),
        Err(e) => return output::print_tool_error(&e, json_mode),
    }

    Ok(())
}

/// Normalize document-type filters to lowercase and validate them against the
/// whitelist of supported types ([`VALID_DOC_TYPES`]).
///
/// Returns:
/// - `Ok(None)` when no filter was provided
/// - `Ok(Some(normalized))` when every type is valid (lowercased, order preserved)
/// - `Err(message)` listing the unknown type(s) and the supported set, ready to
///   surface to the user
fn validate_doc_types(doc_types: Option<Vec<String>>) -> Result<Option<Vec<String>>, String> {
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

#[cfg(test)]
mod tests {
    use super::*;

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
