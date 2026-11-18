//! `linkly pages save|list|get|update|delete` — the `pages` tool: publish a
//! file or text as a Linkly AI Page, and get, update, list or delete your Pages.
//!
//! The rules live in the gateway; these commands pass the arguments on and
//! print the answer as it comes. A `save` with `--file-path` answers with an
//! upload address: send the file there with `linkly pages upload <address> <file>`
//! (upload.rs).

use std::io::Read;

use anyhow::Result;

use crate::cli::{ConnectionArgs, PagesAction};
use crate::client::McpClient;
use crate::connection::ConnectionInfo;
use crate::output;

/// Read the text from stdin when `--content -`.
const STDIN_SENTINEL: &str = "-";

/// The largest text the gateway takes (16 MiB); one byte more tells "over".
const MAX_CONTENT_BYTES: usize = 16 * 1024 * 1024;

/// The `pages` arguments, in the tool's own names.
#[derive(Default)]
pub struct PagesParams {
    pub action: String,
    pub file_path: Option<String>,
    pub content: Option<String>,
    pub content_type: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub access: Option<String>,
    pub url: Option<String>,
    pub confirm_sensitive: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

impl PagesParams {
    /// The tool call for a `linkly pages` subcommand, and the connection it
    /// goes on. `None` for `upload`, which is not a tool call (upload.rs).
    pub fn from_action(action: PagesAction) -> Option<(Self, ConnectionArgs)> {
        let call = |action: &str| PagesParams {
            action: action.to_string(),
            ..Default::default()
        };
        Some(match action {
            PagesAction::Save {
                file_path,
                content,
                content_type,
                title,
                description,
                access,
                url,
                confirm_sensitive,
                conn,
            } => (
                PagesParams {
                    file_path,
                    content,
                    content_type,
                    title,
                    description,
                    access,
                    url,
                    confirm_sensitive,
                    ..call("save")
                },
                conn,
            ),
            PagesAction::List {
                limit,
                offset,
                conn,
            } => (
                PagesParams {
                    limit,
                    offset,
                    ..call("list")
                },
                conn,
            ),
            PagesAction::Get { url, conn } => (
                PagesParams {
                    url: Some(url),
                    ..call("get")
                },
                conn,
            ),
            PagesAction::Update {
                url,
                title,
                description,
                access,
                conn,
            } => (
                PagesParams {
                    url: Some(url),
                    title,
                    description,
                    access,
                    ..call("update")
                },
                conn,
            ),
            PagesAction::Delete { url, conn } => (
                PagesParams {
                    url: Some(url),
                    ..call("delete")
                },
                conn,
            ),
            PagesAction::Upload { .. } => return None,
        })
    }
}

fn read_content(content: &str) -> Result<String> {
    if content != STDIN_SENTINEL {
        return Ok(content.to_string());
    }
    let mut body = String::new();
    std::io::stdin()
        .take(MAX_CONTENT_BYTES as u64 + 1)
        .read_to_string(&mut body)?;
    if body.len() > MAX_CONTENT_BYTES {
        anyhow::bail!("The text on stdin is over 16 MiB, the largest a Page takes.");
    }
    Ok(body)
}

/// The tool arguments for `params`; `content` already read.
fn arguments(params: &PagesParams, content: Option<String>) -> serde_json::Value {
    let mut args = serde_json::json!({ "action": params.action });
    let mut set = |key: &str, value: Option<serde_json::Value>| {
        if let Some(value) = value {
            args[key] = value;
        }
    };
    set("file_path", params.file_path.clone().map(Into::into));
    set("content", content.map(Into::into));
    set("content_type", params.content_type.clone().map(Into::into));
    set("title", params.title.clone().map(Into::into));
    set("description", params.description.clone().map(Into::into));
    set("access", params.access.clone().map(Into::into));
    set("url", params.url.clone().map(Into::into));
    set(
        "confirm_sensitive",
        params.confirm_sensitive.clone().map(Into::into),
    );
    set("limit", params.limit.map(Into::into));
    set("offset", params.offset.map(Into::into));
    args
}

pub async fn run(
    client: &McpClient,
    conn: &ConnectionInfo,
    params: PagesParams,
    json_mode: bool,
) -> Result<()> {
    let content = match params.content.as_deref().map(read_content).transpose() {
        Ok(content) => content,
        Err(e) => return output::print_error(&e.to_string(), json_mode),
    };
    let args = arguments(&params, content);
    match client.call_tool("pages", args, conn).await {
        Ok(text) => output::print_result(&text, json_mode),
        Err(e) if is_unknown_tool(&e) => return output::print_error(NO_PAGES_TOOL, json_mode),
        Err(e) => return output::print_tool_error(&e, json_mode),
    }
    Ok(())
}

/// The connection has no `pages`: a Desktop older than the tool, or the
/// account does not have Linkly AI Pages. The version gate is not raised
/// for it (that would stop every command on older Desktops).
const NO_PAGES_TOOL: &str = "This connection has no pages tool. Update the Linkly AI desktop app \
     to the latest version; if it is up to date, Linkly AI Pages is not available for this account.";

fn is_unknown_tool(err: &anyhow::Error) -> bool {
    err.downcast_ref::<crate::client::ToolError>()
        .is_some_and(|e| {
            let message = e.message.to_ascii_lowercase();
            message.contains("tool not found") || message.contains("unknown tool")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(action: &str) -> PagesParams {
        PagesParams {
            action: action.into(),
            file_path: None,
            content: None,
            content_type: None,
            title: None,
            description: None,
            access: None,
            url: None,
            confirm_sensitive: None,
            limit: None,
            offset: None,
        }
    }

    #[test]
    fn passes_only_what_was_given() {
        let mut save = params("save");
        save.file_path = Some("/Users/me/q3.html".into());
        save.description = Some("Q3 review".into());
        assert_eq!(
            arguments(&save, None),
            serde_json::json!({"action": "save", "file_path": "/Users/me/q3.html", "description": "Q3 review"})
        );
        let mut confirm = params("save");
        confirm.url = Some("https://alicia.linklyai.page/q3-k2m4".into());
        confirm.confirm_sensitive = Some("0d4e3a9c-1111-4000-8000-000000000003".into());
        assert_eq!(
            arguments(&confirm, None),
            serde_json::json!({"action": "save", "url": "https://alicia.linklyai.page/q3-k2m4", "confirm_sensitive": "0d4e3a9c-1111-4000-8000-000000000003"})
        );
        let mut list = params("list");
        list.limit = Some(5);
        assert_eq!(
            arguments(&list, None),
            serde_json::json!({"action": "list", "limit": 5})
        );
    }

    /// The tool arguments `linkly <args>` sends; `None` when it is no tool call.
    fn call(args: &[&str]) -> Option<serde_json::Value> {
        use clap::Parser;
        let cli =
            crate::cli::Cli::try_parse_from(std::iter::once("linkly").chain(args.iter().copied()))
                .expect("parse");
        let crate::cli::Command::Pages { action } = cli.command else {
            panic!("expected a pages command");
        };
        PagesParams::from_action(action).map(|(params, _)| {
            let content = params.content.clone();
            arguments(&params, content)
        })
    }

    #[test]
    fn subcommands_name_the_action() {
        let page = "https://alicia.linklyai.page/q3-k2m4";
        assert_eq!(
            call(&[
                "pages",
                "save",
                "--content",
                "# Hi",
                "--content-type",
                "markdown"
            ]),
            Some(
                serde_json::json!({"action": "save", "content": "# Hi", "content_type": "markdown"})
            )
        );
        assert_eq!(
            call(&["pages", "list", "--limit", "5"]),
            Some(serde_json::json!({"action": "list", "limit": 5}))
        );
        assert_eq!(
            call(&["pages", "get", "--url", page]),
            Some(serde_json::json!({"action": "get", "url": page}))
        );
        assert_eq!(
            call(&["pages", "update", "--url", page, "--access", "public"]),
            Some(serde_json::json!({"action": "update", "url": page, "access": "public"}))
        );
        assert_eq!(
            call(&["pages", "delete", "--url", page]),
            Some(serde_json::json!({"action": "delete", "url": page}))
        );
        // upload talks to the upload address, not to the tool.
        assert_eq!(
            call(&[
                "pages",
                "upload",
                "https://api.linkly.ai/v1/uploads/upl_x",
                "q3.html"
            ]),
            None
        );
    }
}
