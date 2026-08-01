//! Resolving document IDs given on the command line, including reading them
//! from stdin so `search` can feed the commands that take IDs.
//!
//! The whole point of having a CLI next to the MCP server is that its output
//! can be piped into its own input. Without this, the natural pipeline
//!
//! ```text
//! linkly search "x" --json | jq -r '.results[].doc_id' | linkly outline -
//! ```
//!
//! has to be written as a shell loop that calls the CLI once per ID.

use std::io::{BufRead, Read};

use anyhow::Result;

/// Reads IDs from stdin instead of argv, following the usual convention.
pub const STDIN_SENTINEL: &str = "-";

/// Upper bound on how many IDs one invocation will accept from stdin.
///
/// Piping an unfiltered file in by mistake should fail with a message rather
/// than spend an hour issuing requests. Well past any realistic `search`
/// result set.
const MAX_IDS: usize = 10_000;

/// Expand `-` into the IDs on stdin; otherwise return what was given.
///
/// Blank lines and surrounding whitespace are dropped, so output from `jq -r`
/// (which ends with a newline) and hand-typed input both work.
pub fn resolve(ids: &[String]) -> Result<Vec<String>> {
    if !ids.iter().any(|id| id == STDIN_SENTINEL) {
        return Ok(ids.to_vec());
    }
    if ids.len() > 1 {
        anyhow::bail!(
            "'-' reads all IDs from stdin, so it cannot be combined with IDs on the command line.\n\
             Pass either '-' alone or the IDs directly."
        );
    }
    read_ids(std::io::stdin().lock())
}

/// Parse one ID per line, rejecting an input that is empty or implausibly long.
fn read_ids(reader: impl Read) -> Result<Vec<String>> {
    let mut ids = Vec::new();
    for line in std::io::BufReader::new(reader).lines() {
        let line = line.map_err(|e| anyhow::anyhow!("Failed to read IDs from stdin: {}", e))?;
        let id = line.trim();
        if id.is_empty() {
            continue;
        }
        if ids.len() == MAX_IDS {
            anyhow::bail!(
                "Refusing to process more than {} IDs from stdin. \
                 Narrow the search, or split the input.",
                MAX_IDS
            );
        }
        ids.push(id.to_string());
    }
    if ids.is_empty() {
        anyhow::bail!(
            "No document IDs on stdin.\n\
             Expected one ID per line, e.g. `linkly search \"x\" --json | jq -r '.results[].doc_id' | linkly outline -`"
        );
    }
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn ids_given_on_the_command_line_pass_through() {
        let given = s(&["local://1", "local://2"]);
        assert_eq!(resolve(&given).unwrap(), given);
    }

    #[test]
    fn a_dash_among_other_ids_is_rejected_rather_than_guessed() {
        // Merging stdin with argv has no obvious ordering, and silently
        // ignoring one of the two would lose documents.
        let err = resolve(&s(&["local://1", "-"])).unwrap_err().to_string();
        assert!(err.contains("cannot be combined"), "got: {err}");
    }

    #[test]
    fn one_id_per_line_is_parsed() {
        let input = "local://1\nlocal://2\nlocal://3\n";
        assert_eq!(
            read_ids(input.as_bytes()).unwrap(),
            s(&["local://1", "local://2", "local://3"])
        );
    }

    #[test]
    fn blank_lines_and_padding_are_ignored() {
        // `jq -r` output ends with a newline, and hand-edited lists pick up
        // stray spaces.
        let input = "  local://1  \n\n\tlocal://2\n   \n";
        assert_eq!(
            read_ids(input.as_bytes()).unwrap(),
            s(&["local://1", "local://2"])
        );
    }

    #[test]
    fn cloud_ids_containing_spaces_survive_intact() {
        // A cloud doc_id embeds the file path, which routinely contains
        // spaces — splitting on whitespace instead of newlines would corrupt it.
        let input = "cloud://o/s/hash/My Notes/a b.md\n";
        assert_eq!(
            read_ids(input.as_bytes()).unwrap(),
            s(&["cloud://o/s/hash/My Notes/a b.md"])
        );
    }

    #[test]
    fn empty_stdin_is_an_error_with_a_usable_example() {
        let err = read_ids("".as_bytes()).unwrap_err().to_string();
        assert!(err.contains("No document IDs"), "got: {err}");
        assert!(
            err.contains("jq -r"),
            "message should show the idiom: {err}"
        );
    }

    #[test]
    fn whitespace_only_stdin_is_treated_as_empty() {
        assert!(read_ids("\n\n   \n".as_bytes()).is_err());
    }

    #[test]
    fn absurdly_long_input_is_refused_rather_than_processed() {
        let input = "local://1\n".repeat(MAX_IDS + 1);
        let err = read_ids(input.as_bytes()).unwrap_err().to_string();
        assert!(err.contains("Refusing to process"), "got: {err}");
    }
}
