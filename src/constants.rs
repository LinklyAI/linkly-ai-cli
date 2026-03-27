/// Supported document types for search filtering.
/// Matches the whitelist in the desktop app's scanner.
/// Note: CLI commands validate against this list, but MCP bridge passes doc_types
/// through without validation (to stay forward-compatible with new types).
pub const VALID_DOC_TYPES: &[&str] = &["pdf", "docx", "md", "txt", "html", "image"];
