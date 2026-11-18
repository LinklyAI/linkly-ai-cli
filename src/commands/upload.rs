//! `linkly pages upload <address> <file>` — send a file's raw bytes to a Linkly AI
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

use crate::{http, output};

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
    let (status, text) = match send(address, file).await {
        Ok(answer) => answer,
        Err(message) => return output::print_error(&message, json_mode),
    };
    let answer: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
    if !status.is_success() {
        let message = answer
            .get("message")
            .and_then(|m| m.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| format!("HTTP {status}"));
        let code = answer
            .get("error")
            .and_then(|c| c.as_str())
            .unwrap_or("upload_failed");
        return output::print_error(&format!("{code}: {message}"), json_mode);
    }
    if !is_upload_answer(&answer) {
        return output::print_error(
            "The upload's answer could not be read. Retry the same command: the address answers again.",
            json_mode,
        );
    }
    if json_mode {
        // The answer's own `status` (published, needs_confirmation,
        // awaiting_publish, superseded) is the result: printed as it came.
        output::print_answer(&text);
    } else {
        // The answer's `message` says what happened and what to tell the user.
        let message = answer
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or(&text);
        println!("{message}");
    }
    Ok(())
}

/// Whether a successful PUT's body is the endpoint's answer: an object with
/// its `status`. Anything else was garbled on the way.
fn is_upload_answer(answer: &serde_json::Value) -> bool {
    answer
        .get("status")
        .and_then(|status| status.as_str())
        .is_some()
}

/// PUT `file` to `address`: the endpoint's status and its whole answer, or
/// what to tell the user when there is none.
async fn send(address: &str, file: &Path) -> Result<(reqwest::StatusCode, String), String> {
    let size = match tokio::fs::metadata(file).await {
        Ok(meta) if meta.is_file() => meta.len(),
        Ok(_) => return Err(format!("{} is not a file", file.display())),
        Err(e) => return Err(format!("Cannot read {}: {e}", file.display())),
    };
    let handle = tokio::fs::File::open(file)
        .await
        .map_err(|e| format!("Cannot read {}: {e}", file.display()))?;
    let client = http::client_builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(UPLOAD_TIMEOUT)
        .build()
        .map_err(|e| format!("Upload failed: {e}"))?;
    let response = client
        .put(address)
        .header(reqwest::header::CONTENT_LENGTH, size)
        .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
        .body(reqwest::Body::from(handle))
        .send()
        .await
        .map_err(|e| {
            format!(
                "Upload failed: {}. Retry the same command: the address takes the file again until it succeeds.",
                e.without_url()
            )
        })?;
    let status = response.status();
    // Cut off on the way back, the answer is lost, not the upload: the same
    // command is answered again.
    let text = response.text().await.map_err(|e| {
        format!(
            "The upload's answer was cut off: {}. Retry the same command: the address answers again.",
            e.without_url()
        )
    })?;
    Ok((status, text))
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

    #[test]
    fn only_an_object_with_a_status_is_an_upload_answer() {
        assert!(is_upload_answer(
            &serde_json::json!({ "status": "published", "url": "https://a.linklyai.page/x" })
        ));
        assert!(!is_upload_answer(&serde_json::Value::Null));
        assert!(!is_upload_answer(&serde_json::json!(["published"])));
        assert!(!is_upload_answer(
            &serde_json::json!({ "message": "Uploaded." })
        ));
    }

    /// Reads one request off `socket`: head, then the body up to Content-Length.
    async fn read_request(socket: &mut tokio::net::TcpStream) {
        use tokio::io::AsyncReadExt;
        let mut request = Vec::new();
        let mut buf = vec![0u8; 64 * 1024];
        let (start, length) = loop {
            let n = socket.read(&mut buf).await.unwrap();
            request.extend_from_slice(&buf[..n]);
            let text = String::from_utf8_lossy(&request).to_ascii_lowercase();
            if let Some(end) = text.find("\r\n\r\n") {
                let length = text[..end]
                    .lines()
                    .find_map(|l| {
                        l.strip_prefix("content-length: ")
                            .map(|v| v.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                break (end + 4, length);
            }
        };
        while request.len() < start + length {
            let n = socket.read(&mut buf).await.unwrap();
            if n == 0 {
                break;
            }
            request.extend_from_slice(&buf[..n]);
        }
    }

    /// A server that takes one upload and writes `response` back.
    async fn answer_once(response: &'static [u8]) -> String {
        use tokio::io::AsyncWriteExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            read_request(&mut socket).await;
            socket.write_all(response).await.unwrap();
        });
        format!("http://{addr}/v1/uploads/upl_test")
    }

    fn temp_file(bytes: &[u8]) -> tempfile::NamedTempFile {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        std::io::Write::write_all(&mut file, bytes).unwrap();
        file
    }

    #[tokio::test]
    async fn an_answer_cut_off_on_the_way_back_is_a_failure_to_retry() {
        let file = temp_file(b"hello pages");
        let url = answer_once(b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 64\r\nconnection: close\r\n\r\n{\"status\":").await;

        let error = send(&url, file.path()).await.unwrap_err();

        assert!(
            error.starts_with("The upload's answer was cut off"),
            "{error}"
        );
        assert!(error.contains("Retry the same command"), "{error}");
    }

    #[tokio::test]
    async fn a_whole_answer_comes_back_as_sent() {
        let file = temp_file(b"hello pages");
        let url = answer_once(b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 31\r\nconnection: close\r\n\r\n{\"status\":\"needs_confirmation\"}").await;

        let (status, text) = send(&url, file.path()).await.expect("answered");

        assert_eq!(status, reqwest::StatusCode::OK);
        assert_eq!(text, r#"{"status":"needs_confirmation"}"#);
    }
}
