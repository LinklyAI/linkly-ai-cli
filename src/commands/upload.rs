//! `linkly upload <address> <file>` — send a file's raw bytes to a Linkly AI
//! Pages upload address.
//!
//! The `pages` tool's `save` answers with an upload address; agents that can
//! run commands upload with this (or curl). It talks to the upload endpoint
//! directly, not to the Desktop or the MCP gateway, and streams the file — it
//! never holds a 100 MB file in memory.
//!
//! The address must be a Linkly AI one: a tool answer an injected document
//! steered could otherwise send a local file anywhere.

use std::path::Path;
use std::time::Duration;

use anyhow::Result;

use crate::output;

/// Every upload address starts with this.
const UPLOAD_PREFIX: &str = "https://api.linkly.ai/v1/uploads/";

/// The whole upload, however slow the line: an upload address is good for 30 minutes.
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// Whether `address` is a Linkly AI upload address: the prefix, then the
/// token (`upl_` + base64url), nothing else.
fn is_upload_address(address: &str) -> bool {
    let Some(token) = address.strip_prefix(UPLOAD_PREFIX) else {
        return false;
    };
    token.starts_with("upl_")
        && token.len() > 4
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

pub async fn run(address: &str, file: &Path, json_mode: bool) -> Result<()> {
    if !is_upload_address(address) {
        return output::print_error(
            &format!(
                "Not a Linkly AI upload address: it must start with {UPLOAD_PREFIX}upl_. \
                 Use the address from the pages tool's save answer."
            ),
            json_mode,
        );
    }
    let size = match tokio::fs::metadata(file).await {
        Ok(meta) if meta.is_file() => meta.len(),
        Ok(_) => return output::print_error(&format!("{} is not a file", file.display()), json_mode),
        Err(e) => return output::print_error(&format!("Cannot read {}: {e}", file.display()), json_mode),
    };
    let handle = match tokio::fs::File::open(file).await {
        Ok(handle) => handle,
        Err(e) => return output::print_error(&format!("Cannot read {}: {e}", file.display()), json_mode),
    };
    let client = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(UPLOAD_TIMEOUT)
        .user_agent(concat!("linkly-cli/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let response = match client
        .put(address)
        .header(reqwest::header::CONTENT_LENGTH, size)
        .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
        .body(reqwest::Body::from(handle))
        .send()
        .await
    {
        Ok(response) => response,
        Err(e) => {
            return output::print_error(
                &format!("Upload failed: {}. Retry the same command: the address takes the file again until it succeeds.", e.without_url()),
                json_mode,
            )
        }
    };
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    let answer: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
    if !status.is_success() {
        let message = answer
            .get("message")
            .and_then(|m| m.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| format!("HTTP {status}"));
        let code = answer.get("error").and_then(|c| c.as_str()).unwrap_or("upload_failed");
        return output::print_error(&format!("{code}: {message}"), json_mode);
    }
    if json_mode {
        output::print_result(&text, true);
    } else {
        // The answer's `message` says what happened and what to tell the user.
        let message = answer.get("message").and_then(|m| m.as_str()).unwrap_or(&text);
        println!("{message}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_linkly_upload_addresses_are_taken() {
        assert!(is_upload_address(
            "https://api.linkly.ai/v1/uploads/upl_q7Vn3kPx9-2bZ0aLmR4tY8wE1sD6fG5hJ3kL9pO0iU2"
        ));
        for address in [
            "http://api.linkly.ai/v1/uploads/upl_abc",
            "https://api.linkly.ai.evil.com/v1/uploads/upl_abc",
            "https://evil.com/?https://api.linkly.ai/v1/uploads/upl_abc",
            "https://api.linkly.ai/v1/uploads/upl_abc/../../x",
            "https://api.linkly.ai/v1/uploads/upl_abc?x=1",
            "https://api.linkly.ai/v1/uploads/",
            "https://api.linkly.ai/v1/uploads/upl_",
            "https://api.linkly.ai/v1/other/upl_abc",
        ] {
            assert!(!is_upload_address(address), "{address}");
        }
    }
}
