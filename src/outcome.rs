//! Whether a command actually found anything, so the process exit code can say
//! so without the caller parsing output.
//!
//! `grep`'s convention: 0 = found, 1 = nothing found, 2 = the command failed.
//! Before this, "no search hits" and "found 30 documents" both exited 0, which
//! made `linkly search x && …` useless and forced scripts to parse `total`.
//!
//! ## Why the markdown path is deliberately lenient
//!
//! In JSON mode we read the count out of the structured response, which is a
//! stable contract. In markdown mode we can only pattern-match the desktop's
//! human-readable header, and that text is not a contract — it can be reworded
//! at any time.
//!
//! So the markdown matcher fails **towards `Found`**: an unrecognised response
//! keeps today's exit-0 behaviour. Reporting "nothing found" for a response we
//! merely failed to parse would be far worse than missing an exit-1 — a script
//! would silently skip work that had results.

/// Did the command produce any results?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Results were produced, or the command has no notion of emptiness
    /// (`read`, `status`, `auth`, …).
    Found,
    /// The command ran fine and definitively matched nothing.
    Empty,
}

impl Outcome {
    /// Process exit code for this outcome. Failures are mapped to 2 by the
    /// caller; this only distinguishes success shapes.
    pub fn exit_code(self) -> i32 {
        match self {
            Outcome::Found => 0,
            Outcome::Empty => 1,
        }
    }
}

/// Which count field carries "how many results" for a given tool response.
#[derive(Debug, Clone, Copy)]
pub enum ResultShape {
    /// `search` — JSON `total`, markdown `Showing top N of M results`.
    Search,
    /// `grep` — JSON `total_matches`, markdown `Found N matches in M documents`.
    Grep,
    /// `find_paths` — JSON `directories` array, markdown `Found N folder(s)`.
    FindPaths,
    /// `list` — JSON `total` (falling back to the `items` array for desktops
    /// that don't emit it), markdown `Showing N of M notes` (scope=notes) or
    /// `Showing N of M files` (scope=folder/library).
    List,
}

/// Classify a tool response as `Found` or `Empty`.
///
/// `json_mode` tells us which rendering to expect; it is not a hint we can
/// second-guess, because the CLI is the one that asked for that format.
pub fn classify(content: &str, shape: ResultShape, json_mode: bool) -> Outcome {
    if json_mode {
        if let Some(outcome) = classify_json(content, shape) {
            return outcome;
        }
        // Asking for JSON does not guarantee getting it: the cloud gateway
        // returns its "No results found for …" sentence as plain text even in
        // JSON mode. Fall through to the markdown rules before giving up, so a
        // cloud miss is still reported as empty.
    }
    classify_markdown(content, shape).unwrap_or(Outcome::Found)
}

/// Read the count out of a structured response. `None` = could not tell.
fn classify_json(content: &str, shape: ResultShape) -> Option<Outcome> {
    let value: serde_json::Value = serde_json::from_str(content).ok()?;
    let empty = match shape {
        ResultShape::Search => value.get("total")?.as_u64()? == 0,
        ResultShape::Grep => value.get("total_matches")?.as_u64()? == 0,
        ResultShape::FindPaths => value.get("directories")?.as_array()?.is_empty(),
        // The TOTAL decides, same as the markdown path (and `search`): with
        // `offset` past the end, `items` is empty while the filtered set is
        // not — that page must still exit 0. `items` is only a fallback for
        // desktops that don't emit `total`.
        ResultShape::List => match value.get("total").and_then(|t| t.as_u64()) {
            Some(total) => total == 0,
            None => value.get("items")?.as_array()?.is_empty(),
        },
    };
    Some(if empty {
        Outcome::Empty
    } else {
        Outcome::Found
    })
}

/// Read the count out of the desktop's markdown header. `None` = could not
/// tell, which the caller turns into `Found`.
fn classify_markdown(content: &str, shape: ResultShape) -> Option<Outcome> {
    // The cloud gateway renders its own markdown and does not reuse the
    // desktop's header, so an empty cloud search says this instead of
    // "Showing top 0 of 0 results". Checked before the header parse because
    // there is no count to read in that rendering at all.
    if matches!(shape, ResultShape::Search) && content.contains("No results found for") {
        return Some(Outcome::Empty);
    }
    let count = match shape {
        // "Showing top 3 of 42 results for \"query\"" — the *total* is what
        // matters, not the page size.
        ResultShape::Search => count_after(content, "Showing top ", " of ", " results")?,
        // Two renderings, and only the default one has the "Found" header:
        //   output_mode=content → "Found 5 matches in 1 documents for `p`"
        //   output_mode=count   → per-doc lines, then "Total: 5 matches in 1 documents"
        // The count mode is the one scripts reach for as an existence check, so
        // missing it would defeat the exit code exactly where it matters most.
        ResultShape::Grep => match count_after(content, "Found ", "Found ", " matches") {
            Some(count) => count,
            None => count_after(content, "Total: ", "Total: ", " matches")?,
        },
        // "Found 2 folder(s), 940 files total." — the empty case prints a
        // dedicated sentence with no count at all, so a missing header here is
        // itself the signal.
        ResultShape::FindPaths => match count_after(content, "Found ", "Found ", " folder(s)") {
            Some(count) => count,
            None if content.contains("No folder candidates matched") => 0,
            None => return None,
        },
        // Two renderings by scope, same shape:
        //   notes          → "Showing 10 of 37 notes (sort: recent, offset: 0) — target: …"
        //   folder/library → "Showing 50 of 214 files (sort: recent, offset: 0) — target: …"
        // Without the second arm, folder/library results would fall through to
        // the lenient default and --exit-code would degrade to always-Found.
        // Both arms are anchored to the "Showing " header: the notes arm runs
        // first over the whole body, so without the anchor a folder listing
        // containing a file named e.g. "Summary of 0 notes.md" would satisfy
        // the notes pattern from an ITEM line and report a false Empty.
        ResultShape::List => match count_after(content, "Showing ", " of ", " notes") {
            Some(count) => count,
            None => count_after(content, "Showing ", " of ", " files")?,
        },
    };
    Some(if count == 0 {
        Outcome::Empty
    } else {
        Outcome::Found
    })
}

/// Extract the integer sitting between `prefix` and `suffix` on the first line
/// that **starts with** `anchor` and contains both, in that order.
///
/// The anchor is what keeps user-controlled text out of the parse: item lines
/// echo file names, note snippets and matched document lines verbatim, so a
/// free scan over the whole body would let a file named "Summary of 0
/// notes.md" pose as a count header. Desktop headers start the line
/// ("Showing …", "Found …", "Total: …"); item renderings never do.
fn count_after(content: &str, anchor: &str, prefix: &str, suffix: &str) -> Option<u64> {
    for line in content.lines() {
        if !line.starts_with(anchor) {
            continue;
        }
        let Some(rest) = line.split_once(prefix).map(|(_, rest)| rest) else {
            continue;
        };
        let Some((digits, _)) = rest.split_once(suffix) else {
            continue;
        };
        if let Ok(count) = digits.trim().parse::<u64>() {
            return Some(count);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── JSON mode ────────────────────────────────────────

    #[test]
    fn json_search_zero_total_is_empty() {
        let body = r#"{"query":"x","total":0,"results":[]}"#;
        assert_eq!(classify(body, ResultShape::Search, true), Outcome::Empty);
    }

    #[test]
    fn json_search_with_results_is_found() {
        let body = r#"{"query":"x","total":3,"results":[{"doc_id":"local://1"}]}"#;
        assert_eq!(classify(body, ResultShape::Search, true), Outcome::Found);
    }

    #[test]
    fn json_grep_uses_total_matches() {
        assert_eq!(
            classify(r#"{"total_matches":0}"#, ResultShape::Grep, true),
            Outcome::Empty
        );
        assert_eq!(
            classify(r#"{"total_matches":7}"#, ResultShape::Grep, true),
            Outcome::Found
        );
    }

    #[test]
    fn json_find_paths_uses_directories_array() {
        assert_eq!(
            classify(r#"{"directories":[]}"#, ResultShape::FindPaths, true),
            Outcome::Empty
        );
        assert_eq!(
            classify(
                r#"{"directories":[{"path":"/a"}]}"#,
                ResultShape::FindPaths,
                true
            ),
            Outcome::Found
        );
    }

    #[test]
    fn json_list_total_governs_not_the_page() {
        // offset past the end: the page is empty but the filtered set isn't.
        // The markdown rendering of the same query reads the total ("Showing
        // 0 of 214 files") — the JSON path must agree, or --exit-code flips
        // between output formats.
        let past_end = r#"{"scope":"folder","total":214,"items":[]}"#;
        assert_eq!(classify(past_end, ResultShape::List, true), Outcome::Found);

        let truly_empty = r#"{"scope":"folder","total":0,"items":[]}"#;
        assert_eq!(
            classify(truly_empty, ResultShape::List, true),
            Outcome::Empty
        );
    }

    #[test]
    fn json_list_uses_items_array() {
        assert_eq!(
            classify(r#"{"scope":"notes","items":[]}"#, ResultShape::List, true),
            Outcome::Empty
        );
        assert_eq!(
            classify(
                r#"{"scope":"notes","items":[{"note_id":"x"}]}"#,
                ResultShape::List,
                true
            ),
            Outcome::Found
        );
    }

    // ── Markdown mode ────────────────────────────────────

    #[test]
    fn markdown_search_reads_the_total_not_the_page_size() {
        let empty = "# Search Results\n\nShowing top 0 of 0 results for \"zzz\"\n";
        assert_eq!(classify(empty, ResultShape::Search, false), Outcome::Empty);

        // Page size 3 but 42 total — still Found.
        let found = "# Search Results\n\nShowing top 3 of 42 results for \"x\"\n";
        assert_eq!(classify(found, ResultShape::Search, false), Outcome::Found);
    }

    #[test]
    fn markdown_grep_count_mode_uses_the_total_line() {
        // `--mode count` renders no "Found N matches" header at all — it ends
        // with a "Total:" summary instead.
        let empty = "# Grep Results\n\n\nTotal: 0 matches in 0 documents\n";
        assert_eq!(classify(empty, ResultShape::Grep, false), Outcome::Empty);

        let found = "# Grep Results\n\nlocal://1  A doc  3\n\nTotal: 3 matches in 1 documents\n";
        assert_eq!(classify(found, ResultShape::Grep, false), Outcome::Found);
    }

    #[test]
    fn markdown_grep_content_mode_with_matches_is_found() {
        let body = "# Grep Results\n\nFound 5 matches in 2 documents for `x`\n";
        assert_eq!(classify(body, ResultShape::Grep, false), Outcome::Found);
    }

    #[test]
    fn markdown_list_with_notes_is_found() {
        let body = "# Notes\n\nShowing 10 of 37 notes (sort: recent, offset: 0) — target: notes\n";
        assert_eq!(classify(body, ResultShape::List, false), Outcome::Found);
    }

    #[test]
    fn json_mode_falls_back_to_markdown_rules_for_plain_text() {
        // The cloud gateway answers an empty search with plain text even when
        // JSON was requested. Without the fallback this would classify as
        // Found and exit 0 on a genuine miss.
        let body = "No results found for \"zzz\"";
        assert_eq!(classify(body, ResultShape::Search, true), Outcome::Empty);
    }

    #[test]
    fn markdown_cloud_gateway_empty_search_is_empty() {
        // The gateway renders its own text for a cloud-library search that
        // matched nothing — it never emits the desktop's "Showing top N of M"
        // header, so this phrasing is the only signal available.
        let body = "No results found for \"deep work\"";
        assert_eq!(classify(body, ResultShape::Search, false), Outcome::Empty);
    }

    #[test]
    fn markdown_grep_zero_matches_is_empty() {
        let body = "# Grep Results\n\nFound 0 matches in 0 documents for `zzz`\n";
        assert_eq!(classify(body, ResultShape::Grep, false), Outcome::Empty);
    }

    #[test]
    fn markdown_find_paths_recognises_the_dedicated_empty_sentence() {
        let body =
            "# Find Paths\n\nNo folder candidates matched. The keywords may match only filenames…";
        assert_eq!(
            classify(body, ResultShape::FindPaths, false),
            Outcome::Empty
        );
    }

    #[test]
    fn markdown_find_paths_with_candidates_is_found() {
        let body = "# Find Paths\n\nFound 2 folder(s), 940 files total.\n";
        assert_eq!(
            classify(body, ResultShape::FindPaths, false),
            Outcome::Found
        );
    }

    #[test]
    fn markdown_list_zero_notes_is_empty() {
        let body = "# Notes\n\nShowing 0 of 0 notes (sort: recent, offset: 0) — target: notes\n";
        assert_eq!(classify(body, ResultShape::List, false), Outcome::Empty);
    }

    #[test]
    fn markdown_list_empty_page_of_nonempty_total_is_found() {
        // Same offset-past-the-end shape as the JSON test above: the total
        // (214) decides, not the page size (0).
        let body =
            "# Files\n\nShowing 0 of 214 files (sort: recent, offset: 5000) — target: /Users/me\n";
        assert_eq!(classify(body, ResultShape::List, false), Outcome::Found);
    }

    #[test]
    fn markdown_list_files_header_is_recognised() {
        // scope=folder/library renders "files" where notes renders "notes"
        // (#141 PR-2); both must classify, or folder/library listings would
        // silently lose their exit-code semantics.
        let found =
            "# Files\n\nShowing 50 of 214 files (sort: recent, offset: 0) — target: /Users/me\n";
        assert_eq!(classify(found, ResultShape::List, false), Outcome::Found);

        let empty =
            "# Files\n\nShowing 0 of 0 files (sort: recent, offset: 0) — target: /Users/me\n";
        assert_eq!(classify(empty, ResultShape::List, false), Outcome::Empty);
    }

    #[test]
    fn markdown_list_item_lines_cannot_hijack_the_count() {
        // A folder listing echoes user file names verbatim. A file whose name
        // happens to contain " of 0 notes" must not satisfy the notes-header
        // pattern and turn a 214-file listing into exit 1 — only the
        // line-initial "Showing …" header carries the count.
        let body = "# Files\n\n\
                    Showing 50 of 214 files (sort: recent, offset: 0) — target: /Users/me\n\n\
                    - Summary of 0 notes.md (/Users/me/docs)\n";
        assert_eq!(classify(body, ResultShape::List, false), Outcome::Found);
    }

    #[test]
    fn markdown_list_poisoned_item_without_a_header_fails_towards_found() {
        // No recognisable header at all: the poisoned item line must not be
        // parsed in its place. Unparseable → Found, per the module contract.
        let body = "# Files\n\n- Summary of 0 notes.md (/Users/me/docs)\n";
        assert_eq!(classify(body, ResultShape::List, false), Outcome::Found);
    }

    #[test]
    fn markdown_grep_matched_lines_cannot_hijack_the_count() {
        // grep echoes matched document lines; a document that *contains* the
        // header phrasing mid-line must not override the real header above it.
        let body = "# Grep Results\n\n\
                    Found 2 matches in 1 documents for `notes`\n\n\
                    12: …they said Found 0 matches in 0 documents somewhere…\n";
        assert_eq!(classify(body, ResultShape::Grep, false), Outcome::Found);
    }

    // ── Failing towards Found ────────────────────────────

    #[test]
    fn unparseable_markdown_is_treated_as_found() {
        // The desktop reworded its header, or this is some future format.
        // Falling back to Empty here would make scripts skip real work.
        let body = "# Search Results\n\nHere are your 5 documents:\n";
        assert_eq!(classify(body, ResultShape::Search, false), Outcome::Found);
    }

    #[test]
    fn unparseable_json_is_treated_as_found() {
        assert_eq!(
            classify("not json at all", ResultShape::Search, true),
            Outcome::Found
        );
    }

    #[test]
    fn json_missing_the_count_field_is_treated_as_found() {
        // An older desktop that doesn't emit `total`.
        assert_eq!(
            classify(r#"{"results":[]}"#, ResultShape::Search, true),
            Outcome::Found
        );
    }

    #[test]
    fn exit_codes_follow_the_grep_convention() {
        assert_eq!(Outcome::Found.exit_code(), 0);
        assert_eq!(Outcome::Empty.exit_code(), 1);
    }
}
