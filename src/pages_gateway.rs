//! Linkly AI Pages on the cloud gateway, with the CLI's own API key.
//!
//! `pages` runs in the gateway alone, so a call is one authenticated JSON-RPC
//! POST: no session, no health check, and no Desktop.

use std::time::Duration;

use anyhow::{bail, Result};
use serde_json::{json, Value};

use crate::client::{ToolError, ToolFailure};
use crate::connection::{remote_api_key, sanitize_client_name, CLIENT_HEADER, REMOTE_MCP_URL};

/// A save with text checks and stores it before it answers.
const CALL_TIMEOUT: Duration = Duration::from_secs(120);

/// What the caller is called when it has no name of its own.
const DEFAULT_CLIENT: &str = "cli";

const NO_KEY: &str =
    "Sign in with `linkly auth login` or set LINKLY_API_KEY to use Linkly AI Pages.";
const KEY_REJECTED: &str = "Your API key was rejected. Sign in again with `linkly auth login`.";
pub const NOT_AVAILABLE: &str = "Linkly AI Pages is not available for this account.";

pub struct PagesGateway {
    url: String,
    api_key: String,
    /// `--client`, which goes before the name of the application a bridge serves.
    client: Option<String>,
    /// `--remote`: never matched to a Page by the path of its file.
    remote: bool,
}

impl PagesGateway {
    /// The gateway with the saved key or `LINKLY_API_KEY`.
    pub fn from_credentials(remote: bool, client: Option<&str>) -> Result<Self> {
        let Some(api_key) = remote_api_key()? else {
            bail!("{NO_KEY}");
        };
        Ok(Self {
            url: REMOTE_MCP_URL.to_string(),
            api_key,
            client: client.and_then(sanitize_client_name),
            remote,
        })
    }

    #[cfg(test)]
    pub(crate) fn at(url: String, remote: bool, client: Option<&str>) -> Self {
        Self {
            url,
            api_key: "lkai_0123456789abcdef0123456789abcdef".to_string(),
            client: client.map(str::to_string),
            remote,
        }
    }

    /// Call `pages` and return its text. `caller` names the application a
    /// bridge works for; `--client` goes before it.
    pub async fn call(&self, args: Value, caller: Option<&str>) -> Result<String> {
        let device_id = match args.get("file_path") {
            Some(Value::String(_)) if !self.remote => crate::device_id::for_pages(),
            _ => None,
        };
        let client = self.client.as_deref().or(caller).unwrap_or(DEFAULT_CLIENT);
        let mut request = self
            .post("tools/call", json!({ "name": "pages", "arguments": args }))?
            .timeout(CALL_TIMEOUT)
            .header(CLIENT_HEADER, client);
        if !self.remote {
            request = request.header("X-Linkly-Connection", "local");
        }
        if let Some(id) = device_id {
            request = request.header("X-Linkly-Pages-Device-Id", id);
        }
        let result = send(request).await?;
        let text = result["content"]
            .as_array()
            .map(|blocks| {
                blocks
                    .iter()
                    .filter_map(|block| block["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .ok_or_else(|| anyhow::anyhow!("Linkly AI Pages gave an answer without content."))?;
        if result["isError"].as_bool() == Some(true) {
            return Err(ToolFailure { text }.into());
        }
        Ok(text)
    }

    /// Whether the account has Linkly AI Pages: the gateway lists the tool for it.
    pub async fn lists_pages(&self, timeout: Duration) -> Result<bool> {
        let result = send(self.post("tools/list", json!({}))?.timeout(timeout)).await?;
        Ok(result["tools"]
            .as_array()
            .is_some_and(|tools| tools.iter().any(|tool| tool["name"] == "pages")))
    }

    fn post(&self, method: &str, params: Value) -> Result<reqwest::RequestBuilder> {
        let builder = crate::http::client_builder();
        // A fake gateway on this machine is never behind the environment's proxy.
        #[cfg(test)]
        let builder = builder.no_proxy();
        Ok(builder
            .build()?
            .post(&self.url)
            .bearer_auth(&self.api_key)
            .header("Accept", "application/json, text/event-stream")
            .json(&json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params })))
    }
}

/// Send once — a write is never repeated on its own — and return the JSON-RPC result.
async fn send(request: reqwest::RequestBuilder) -> Result<Value> {
    let response = request.send().await.map_err(network_error)?;
    let status = response.status();
    let text = response.text().await.map_err(network_error)?;
    let body: Option<Value> = serde_json::from_str(&text).ok();
    if !status.is_success() {
        if status == reqwest::StatusCode::UNAUTHORIZED {
            bail!("{KEY_REJECTED}");
        }
        match body.as_ref().and_then(refusal) {
            Some(reason) => bail!("Linkly AI Pages refused the call ({status}): {reason}"),
            None => bail!("Linkly AI Pages answered with {status}. Try again later."),
        }
    }
    let Some(mut body) = body else {
        bail!("Linkly AI Pages gave an answer that is not JSON. Try again later.");
    };
    if let Some(error) = body.get("error") {
        return Err(ToolError {
            code: error["code"].as_i64().unwrap_or_default() as i32,
            message: error["message"]
                .as_str()
                .unwrap_or("unknown error")
                .to_string(),
            data: error.get("data").cloned(),
        }
        .into());
    }
    Ok(body["result"].take())
}

/// The reason in an error body, whichever way the gateway wrote it.
fn refusal(body: &Value) -> Option<&str> {
    [
        &body["error"]["message"],
        &body["error_description"],
        &body["message"],
        &body["error"],
    ]
    .into_iter()
    .find_map(Value::as_str)
}

fn network_error(err: reqwest::Error) -> anyhow::Error {
    if err.is_timeout() {
        return anyhow::anyhow!(
            "Linkly AI Pages did not answer in time. The call may still have gone through: run \
             `linkly pages list` before trying again."
        );
    }
    anyhow::anyhow!(
        "Could not reach Linkly AI Pages: {}. Check your network connection.",
        err.without_url()
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Each request a fake gateway got: its head in lower case, and its body.
    pub(crate) type Requests = Arc<Mutex<Vec<(String, Value)>>>;

    /// A gateway on this machine that gives the answers in order, then the last one again.
    pub(crate) async fn fake_gateway(answers: Vec<(&'static str, Value)>) -> (String, Requests) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind a loopback port");
        let url = format!("http://{}/mcp", listener.local_addr().expect("address"));
        let requests: Requests = Arc::default();
        let seen = requests.clone();
        tokio::spawn(async move {
            let mut turn = 0;
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut raw = Vec::new();
                let (head, body) = loop {
                    let mut chunk = [0u8; 4096];
                    let n = stream.read(&mut chunk).await.expect("read");
                    raw.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&raw).into_owned();
                    let Some((head, body)) = text.split_once("\r\n\r\n") else {
                        continue;
                    };
                    if let Ok(body) = serde_json::from_str::<Value>(body) {
                        break (head.to_ascii_lowercase(), body);
                    }
                };
                seen.lock().unwrap().push((head, body));
                let (status, answer) = &answers[turn.min(answers.len() - 1)];
                turn += 1;
                let answer = answer.to_string();
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}",
                    answer.len()
                );
                stream.write_all(response.as_bytes()).await.expect("write");
            }
        });
        (url, requests)
    }

    pub(crate) fn text_answer(text: &str) -> (&'static str, Value) {
        (
            "200 OK",
            json!({ "jsonrpc": "2.0", "id": 1, "result": { "content": [{ "type": "text", "text": text }] } }),
        )
    }

    #[tokio::test]
    async fn one_post_carries_the_call_the_key_and_the_caller() {
        let text = "Upload within 30 minutes:\n  PUT https://api.linkly.ai/v1/uploads/upl_x\nPassword: abc";
        let (url, requests) = fake_gateway(vec![text_answer(text)]).await;
        let gateway = PagesGateway::at(url, false, None);

        let answer = gateway
            .call(json!({"action": "list"}), Some("claude-code"))
            .await
            .expect("an answer");

        assert_eq!(answer, text);
        let seen = requests.lock().unwrap();
        assert_eq!(seen.len(), 1);
        let (head, body) = &seen[0];
        assert!(head.contains("authorization: bearer lkai_"), "{head}");
        assert!(head.contains("x-linkly-client: claude-code"), "{head}");
        assert!(head.contains("x-linkly-connection: local"), "{head}");
        assert!(!head.contains("device-id"), "{head}");
        assert_eq!(body["method"], "tools/call");
        assert_eq!(
            body["params"],
            json!({"name": "pages", "arguments": {"action": "list"}})
        );
    }

    #[tokio::test]
    async fn the_client_flag_goes_before_the_application_and_cli_is_the_default() {
        let (url, requests) = fake_gateway(vec![text_answer("ok")]).await;
        PagesGateway::at(url.clone(), false, Some("my-flow"))
            .call(json!({"action": "list"}), Some("cursor"))
            .await
            .expect("an answer");
        PagesGateway::at(url, false, None)
            .call(json!({"action": "list"}), None)
            .await
            .expect("an answer");
        let seen = requests.lock().unwrap();
        assert!(seen[0].0.contains("x-linkly-client: my-flow"));
        assert!(seen[1].0.contains("x-linkly-client: cli"));
    }

    /// The path of a file is matched to a Page only from this computer, and
    /// never with the header that links a device to an account.
    #[test]
    fn a_file_save_names_the_computer_except_with_remote() {
        crate::test_helpers::with_temp_home("pages_device_headers", |_| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime");
            runtime.block_on(async {
                let (url, requests) = fake_gateway(vec![text_answer("ok")]).await;
                let save = json!({"action": "save", "file_path": "/Users/me/a.md"});
                for remote in [false, true] {
                    PagesGateway::at(url.clone(), remote, None)
                        .call(save.clone(), None)
                        .await
                        .expect("an answer");
                }
                let seen = requests.lock().unwrap();
                assert!(seen[0].0.contains("x-linkly-pages-device-id: "));
                assert!(!seen[0].0.contains("\r\nx-device-id"));
                assert!(!seen[1].0.contains("device-id"));
                assert!(!seen[1].0.contains("x-linkly-connection"));
            });
        });
    }

    #[tokio::test]
    async fn failures_keep_what_the_gateway_said() {
        let (url, _) = fake_gateway(vec![
            (
                "200 OK",
                json!({"jsonrpc": "2.0", "id": 1, "error": {"code": -32602, "message": "Invalid", "data": {"reason": "why"}}}),
            ),
            (
                "200 OK",
                json!({"jsonrpc": "2.0", "id": 1, "result": {"isError": true, "content": [{"type": "text", "text": "Not set up"}]}}),
            ),
            ("401 Unauthorized", json!({})),
            ("403 Forbidden", json!({"error": "insufficient_scope", "error_description": "'mcp' required"})),
            ("503 Service Unavailable", json!({})),
        ])
        .await;
        let gateway = PagesGateway::at(url, false, None);
        let mut errors = Vec::new();
        for _ in 0..5 {
            errors.push(
                gateway
                    .call(json!({"action": "list"}), None)
                    .await
                    .expect_err("a failure"),
            );
        }
        let rpc = errors[0].downcast_ref::<ToolError>().expect("a ToolError");
        assert_eq!(
            (rpc.code, rpc.data.clone()),
            (-32602, Some(json!({"reason": "why"})))
        );
        let failure = errors[1]
            .downcast_ref::<ToolFailure>()
            .expect("a ToolFailure");
        assert_eq!(failure.text, "Not set up");
        assert_eq!(errors[2].to_string(), KEY_REJECTED);
        assert!(errors[3].to_string().contains("'mcp' required"));
        assert!(errors[4].to_string().contains("503"));
    }

    #[tokio::test]
    async fn the_tool_list_says_whether_the_account_has_pages() {
        let list = |names: Value| {
            (
                "200 OK",
                json!({"jsonrpc": "2.0", "id": 1, "result": {"tools": names}}),
            )
        };
        let (url, _) = fake_gateway(vec![
            list(json!([{"name": "search"}, {"name": "pages"}])),
            list(json!([{"name": "search"}])),
            ("401 Unauthorized", json!({})),
        ])
        .await;
        let gateway = PagesGateway::at(url, false, None);
        let timeout = Duration::from_secs(5);
        assert!(gateway.lists_pages(timeout).await.expect("a list"));
        assert!(!gateway.lists_pages(timeout).await.expect("a list"));
        assert!(gateway.lists_pages(timeout).await.is_err());
    }

    #[test]
    fn without_a_key_it_says_how_to_sign_in_whatever_desktop_runs() {
        crate::test_helpers::with_temp_home("pages_no_key", |home| {
            std::fs::create_dir_all(home.join(".linkly")).unwrap();
            std::fs::write(home.join(".linkly").join("port"), r#"{"port": 60606}"#).unwrap();
            let err = PagesGateway::from_credentials(false, None)
                .err()
                .expect("no key");
            assert_eq!(err.to_string(), NO_KEY);
        });
    }
}
