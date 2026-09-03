/// Insert `key` into `args` only when a value is present.
///
/// The absent case must omit the key entirely rather than send `null`: the
/// desktop declares these optionals with `serde(default)` on non-null types,
/// so a literal `null` fails deserialization where omission succeeds. Shared
/// by `search` and `list` so their overlapping filters (`library`,
/// `doc_types`, `modified_after`, …) cannot drift to different encodings.
pub(crate) fn set_optional_arg<T: serde::Serialize>(
    args: &mut serde_json::Value,
    key: &str,
    value: Option<T>,
) {
    if let Some(value) = value {
        args[key] = serde_json::json!(value);
    }
}

pub mod auth;
pub mod completions;
pub mod doctor;
pub mod explore;
pub mod find_paths;
pub mod grep;
pub mod list;
pub mod list_libraries;
pub mod mcp;
pub mod note_save;
pub mod outline;
pub mod read;
pub mod search;
pub mod self_update;
pub mod skills;
pub mod status;
