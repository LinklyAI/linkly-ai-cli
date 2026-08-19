/// Supported document types for search filtering.
///
/// Mirrors `doc_type_from_ext` in the desktop app's scanner — the desktop is the
/// authoritative source and performs **no** validation of its own (`doc_types`
/// goes straight into the search index as a term). This list is therefore the
/// only gate a `--type` value passes through, which cuts both ways: it catches
/// typos before a pointless round-trip, but it also rejects legitimate types
/// whenever the desktop gains a format and this list isn't updated with it.
///
/// **When the desktop adds a document type, add it here in the same change.**
/// Derived types (`image` = OCR text, `audio` / `video` = transcripts) count.
///
/// Note: the MCP bridge (`linkly mcp`) passes `doc_types` through without
/// validation, so an out-of-date CLI never blocks a bridged client.
pub const VALID_DOC_TYPES: &[&str] = &[
    "pdf", "docx", "doc", "pptx", "epub", "rtf", "md", "txt", "html", "image", "audio", "video",
];
