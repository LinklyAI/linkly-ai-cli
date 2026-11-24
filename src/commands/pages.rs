//! `linkly pages save|list|get|update|delete` — the `pages` tool: publish a
//! file or text as a Linkly AI Page, and get, update, list or delete your Pages.
//!
//! The rules live in the gateway; these commands pass the arguments on and
//! print the answer as it comes. Two things are done here: flag combinations
//! the gateway would refuse are named in CLI spelling before the tool call
//! (as `note-save` does), and a `save` with `--file-path` sends the file to
//! the upload address its answer gives, printing the upload's answer the way
//! `linkly pages upload` does (upload.rs), unless `--access` meets a Page that
//! already exists.

use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::cli::{ConnectionArgs, PagesAction};
use crate::client::McpClient;
use crate::commands::upload;
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

/// The flag combinations the gateway refuses, named in CLI spelling. Only the
/// combinations: what the values may be stays the gateway's to check.
fn combination_error(params: &PagesParams) -> Option<&'static str> {
    match params.action.as_str() {
        "save" => save_combination_error(params),
        "update"
            if params.title.is_none()
                && params.description.is_none()
                && params.access.is_none() =>
        {
            Some(
                "update needs at least one of --title, --description, --access. To replace the \
                 content, use `linkly pages save --url <url> --file-path <file>`.",
            )
        }
        _ => None,
    }
}

fn save_combination_error(params: &PagesParams) -> Option<&'static str> {
    let has_file = params.file_path.is_some();
    let has_text = params.content.is_some();
    if has_file && has_text {
        return Some("Pass --file-path or --content, not both.");
    }
    if params.confirm_sensitive.is_some() {
        if params.url.is_none() {
            return Some("--confirm-sensitive needs --url: the Page whose held upload to publish.");
        }
        if has_file || has_text {
            return Some(
                "--confirm-sensitive publishes the upload held on the Page at --url; leave out \
                 --file-path and --content.",
            );
        }
        return None;
    }
    if !has_file && !has_text {
        return Some(if params.url.is_none() {
            "save needs --file-path (a file to publish) or --content (the text itself)."
        } else {
            "save with --url needs --file-path or --content: the new content. To change the \
             title, description or access, use `linkly pages update`."
        });
    }
    if has_text && params.content_type.is_none() {
        return Some("--content needs --content-type: markdown or html.");
    }
    if !has_text && params.content_type.is_some() {
        return Some("--content-type goes with --content.");
    }
    None
}

/// `path` made absolute: an absolute path as it is, a relative one joined to
/// `base` without its `.` parts. `..` stays as written: through a symlinked
/// directory it may lead elsewhere than the shortened path would. An absolute
/// path is never rewritten: the gateway compares paths as spelled.
fn absolute_from(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    base.join(path).components().collect()
}

/// A path the gateway can be given as it is: in UTF-8. A lossy spelling of
/// another one could match the Page of another file.
fn utf8_path(path: PathBuf) -> Result<PathBuf, String> {
    if path.to_str().is_none() {
        return Err(format!(
            "{} is not a UTF-8 path; pass the file's absolute path in UTF-8",
            path.display()
        ));
    }
    Ok(path)
}

/// The file a `save --file-path` publishes, as an absolute path: the Desktop
/// recognizes a file shared again only by its absolute path. A relative path is
/// taken from the current directory as the system resolves it, so through a
/// symlinked directory it is spelled with the real path, unlike an absolute
/// path typed through the symlink: those two make two Pages. It has to be a
/// file on this machine, since this command uploads it.
fn local_file(raw: &str) -> Result<PathBuf, String> {
    let given = Path::new(raw);
    let path = if given.is_absolute() {
        given.to_path_buf()
    } else {
        let base = std::env::current_dir().map_err(|e| format!("Cannot read {raw}: {e}"))?;
        utf8_path(absolute_from(&base, given))?
    };
    match std::fs::metadata(&path) {
        Ok(meta) if meta.is_file() => Ok(path),
        Ok(_) => Err(format!("{} is not a file", path.display())),
        Err(e) => Err(format!("Cannot read {}: {e}", path.display())),
    }
}

/// The line of a save's answer that says it makes a new Page, as the gateway writes it.
const NEW_PAGE_LINE: &str = "Link to share (url), live once uploaded: ";

/// The line of a save's answer for a new Page the Desktop will not recognize by
/// its path next time: another connection, or a file in a temporary folder.
const UPDATE_BY_URL_LINE: &str = "To update this Page later, pass its url with action=save.";

/// The upload address in a save's answer: the one line that reads
/// `PUT <address>` (the gateway keeps it alone on its line). `None` without
/// such a line, or with more than one, which a file path with a line break in
/// it could add: then nothing is sent and the answer is printed as it came.
fn upload_address(answer: &str) -> Option<&str> {
    let mut addresses = answer
        .lines()
        .filter_map(|line| line.trim().strip_prefix("PUT "))
        .filter(|address| upload::is_upload_address(address));
    let address = addresses.next()?;
    addresses.next().is_none().then_some(address)
}

/// Where this command sends the file itself: the upload address of a file
/// save's answer. With `--access`, only for a new Page. Saving into an existing
/// Page never changes its access, which may be wider than the one asked for;
/// the answer says so before anything goes live, so it is printed as it came
/// and the file is sent only after the user has seen it.
fn address_to_upload<'a>(params: &PagesParams, answer: &'a str) -> Option<&'a str> {
    params.file_path.as_ref()?;
    if params.access.is_some() && !answer.lines().any(|line| line.starts_with(NEW_PAGE_LINE)) {
        return None;
    }
    upload_address(answer)
}

/// Send the file a save registered and print the upload's answer, as
/// `linkly pages upload` does. A failure comes with the save's answer, which
/// keeps the address and the other ways to send the file (the browser link
/// among them): running the save again would start another upload.
async fn upload_file(address: &str, file: &Path, save_answer: &str, json_mode: bool) -> Result<()> {
    let retry = format!(
        "Retry with `linkly pages upload {address} \"{}\"`",
        file.display()
    );
    let (text, answer) = match upload::upload(address, file, &retry).await {
        Ok(answer) => answer,
        Err(message) => {
            let message = format!(
                "{message}\n\nThe save's answer, with the upload address and the other ways to send the file:\n{save_answer}"
            );
            return output::print_error(&message, json_mode);
        }
    };
    upload::print_answer(&text, &answer, json_mode);
    if !json_mode && save_answer.lines().any(|line| line == UPDATE_BY_URL_LINE) {
        let url = answer
            .get("url")
            .and_then(|url| url.as_str())
            .unwrap_or("<url>");
        println!("To update this Page later, pass its url: `linkly pages save --url {url} --file-path <file>`.");
    }
    Ok(())
}

pub async fn run(
    client: &McpClient,
    conn: &ConnectionInfo,
    mut params: PagesParams,
    json_mode: bool,
) -> Result<()> {
    // Every check on the flags runs before `--content -` reads stdin, so a
    // mistake never drains a pipe the producer cannot replay (as note-save).
    if let Some(message) = combination_error(&params) {
        return output::print_error(message, json_mode);
    }
    let file = match params.file_path.as_deref().map(local_file).transpose() {
        Ok(file) => file,
        Err(message) => return output::print_error(&message, json_mode),
    };
    if let Some(file) = &file {
        // UTF-8 already (`local_file`): nothing is lost here.
        params.file_path = Some(file.to_string_lossy().into_owned());
    }
    let content = match params.content.as_deref().map(read_content).transpose() {
        Ok(content) => content,
        Err(e) => return output::print_error(&e.to_string(), json_mode),
    };
    let args = arguments(&params, content);
    let text = match client.call_tool("pages", args, conn).await {
        Ok(text) => text,
        Err(e) if is_unknown_tool(&e) => return output::print_error(NO_PAGES_TOOL, json_mode),
        Err(e) => return output::print_tool_error(&e, json_mode),
    };
    match (&file, address_to_upload(&params, &text)) {
        (Some(file), Some(address)) => upload_file(address, file, &text, json_mode).await,
        // Text saved, a confirmation, any other action, `--access` meeting an
        // existing Page, or an answer without an upload address: as it came.
        _ => {
            output::print_result(&text, json_mode);
            Ok(())
        }
    }
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

    #[test]
    fn a_save_needs_a_file_or_the_text() {
        assert_eq!(
            combination_error(&params("save")),
            Some("save needs --file-path (a file to publish) or --content (the text itself).")
        );
        let mut by_url = params("save");
        by_url.url = Some("https://alicia.linklyai.page/q3-k2m4".into());
        assert!(combination_error(&by_url)
            .is_some_and(|message| message.contains("use `linkly pages update`")));
    }

    #[test]
    fn a_save_takes_a_file_or_the_text_not_both() {
        let mut both = params("save");
        both.file_path = Some("/Users/me/q3.md".into());
        both.content = Some("# Q3".into());
        both.content_type = Some("markdown".into());
        assert_eq!(
            combination_error(&both),
            Some("Pass --file-path or --content, not both.")
        );
    }

    #[test]
    fn the_text_and_its_type_go_together() {
        let mut text = params("save");
        text.content = Some("# Q3".into());
        assert_eq!(
            combination_error(&text),
            Some("--content needs --content-type: markdown or html.")
        );
        let mut type_only = params("save");
        type_only.file_path = Some("/Users/me/q3.md".into());
        type_only.content_type = Some("markdown".into());
        assert_eq!(
            combination_error(&type_only),
            Some("--content-type goes with --content.")
        );
    }

    #[test]
    fn a_confirmation_names_the_page_and_nothing_to_upload() {
        let mut no_url = params("save");
        no_url.confirm_sensitive = Some("0d4e3a9c-1111-4000-8000-000000000003".into());
        assert_eq!(
            combination_error(&no_url),
            Some("--confirm-sensitive needs --url: the Page whose held upload to publish.")
        );
        let mut with_file = params("save");
        with_file.confirm_sensitive = Some("0d4e3a9c-1111-4000-8000-000000000003".into());
        with_file.url = Some("https://alicia.linklyai.page/q3-k2m4".into());
        with_file.file_path = Some("/Users/me/q3.md".into());
        assert!(combination_error(&with_file)
            .is_some_and(|message| message.starts_with("--confirm-sensitive publishes")));
    }

    #[test]
    fn an_update_needs_something_to_change() {
        let mut nothing = params("update");
        nothing.url = Some("https://alicia.linklyai.page/q3-k2m4".into());
        assert!(combination_error(&nothing)
            .is_some_and(|message| message.starts_with("update needs at least one of")));
    }

    #[test]
    fn whole_combinations_reach_the_gateway() {
        let mut file = params("save");
        file.file_path = Some("q3.md".into());
        let mut text = params("save");
        text.content = Some("# Q3".into());
        text.content_type = Some("markdown".into());
        let mut confirm = params("save");
        confirm.url = Some("https://alicia.linklyai.page/q3-k2m4".into());
        confirm.confirm_sensitive = Some("0d4e3a9c-1111-4000-8000-000000000003".into());
        let mut update = params("update");
        update.url = Some("https://alicia.linklyai.page/q3-k2m4".into());
        update.access = Some("public".into());
        assert_eq!(combination_error(&file), None);
        assert_eq!(combination_error(&text), None);
        assert_eq!(combination_error(&confirm), None);
        assert_eq!(combination_error(&update), None);
        assert_eq!(combination_error(&params("list")), None);
    }

    #[test]
    fn a_relative_file_path_is_taken_from_the_base_without_its_dots() {
        // Compared as spelled: Path equality would overlook a `.` left in.
        let base = std::env::temp_dir().join("work");
        assert_eq!(
            absolute_from(&base, Path::new("./reports/./q3.html")).as_os_str(),
            base.join("reports").join("q3.html").as_os_str()
        );
        // `..` stays: through a symlinked directory it may name another place.
        assert_eq!(
            absolute_from(&base, Path::new("../q3.html")).as_os_str(),
            base.join("..").join("q3.html").as_os_str()
        );
    }

    #[test]
    fn an_absolute_file_path_is_left_as_written() {
        // The gateway hashes the path as spelled: rewriting it would stop a
        // Page made from it matching.
        let written = std::env::temp_dir().join("a").join(".").join("q3.html");
        assert_eq!(
            absolute_from(Path::new("/elsewhere"), &written).as_os_str(),
            written.as_os_str()
        );
    }

    #[test]
    fn the_file_to_publish_must_be_a_file_here() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = file.path().to_str().unwrap();
        assert_eq!(local_file(path).unwrap(), file.path());
        assert!(local_file(dir.path().to_str().unwrap())
            .is_err_and(|message| message.ends_with("is not a file")));
        assert!(local_file(&format!("{path}.missing"))
            .is_err_and(|message| message.starts_with("Cannot read")));
    }

    /// A save's answer as the gateway writes it, shortened.
    const SAVE_ANSWER: &str = "Upload needed. The Page is not live yet.
Link to share (url), live once uploaded: https://alicia.linklyai.page/q3-growth
Upload within 30 minutes, up to 10 MB:
  PUT https://api.linkly.ai/v1/uploads/upl_q7Vn3kPx9-2bZ0aLmR4tY8wE1sD6fG5hJ3kL9pO0iU2
  Body: the raw bytes of /Users/me/q3-growth.html. Not multipart, not base64.
If you cannot run commands or reach the network, give the user this link to upload the file
in a browser: https://linkly.ai/upload/upl_q7Vn3kPx9-2bZ0aLmR4tY8wE1sD6fG5hJ3kL9pO0iU2";

    #[test]
    fn the_upload_address_is_the_put_line_of_the_answer() {
        assert_eq!(
            upload_address(SAVE_ANSWER),
            Some(
                "https://api.linkly.ai/v1/uploads/upl_q7Vn3kPx9-2bZ0aLmR4tY8wE1sD6fG5hJ3kL9pO0iU2"
            )
        );
    }

    #[test]
    fn an_answer_without_one_put_line_gives_no_address() {
        assert_eq!(
            upload_address("Published: https://alicia.linklyai.page/q3-growth"),
            None
        );
        // A file path with a line break could add a second line: nothing is sent.
        let two = format!("{SAVE_ANSWER}\n  PUT https://api.linkly.ai/v1/uploads/upl_other");
        assert_eq!(upload_address(&two), None);
        let elsewhere = SAVE_ANSWER.replace(
            "PUT https://api.linkly.ai/v1/uploads/",
            "PUT https://uploads.example.com/",
        );
        assert_eq!(upload_address(&elsewhere), None);
    }

    /// The answer of a save into a Page that already exists, `--access` given.
    const UPDATE_ANSWER: &str = "This updates https://alicia.linklyai.page/q3-growth, first shared on 2026-11-10 from the same file. The current content stays live until the upload finishes.
Access stays as it is (public). Saving content never changes access; use action=update for that. The access you passed was not applied.
Upload within 30 minutes, up to 10 MB:
  PUT https://api.linkly.ai/v1/uploads/upl_q7Vn3kPx9-2bZ0aLmR4tY8wE1sD6fG5hJ3kL9pO0iU2
  Body: the raw bytes of /Users/me/q3-growth.html. Not multipart, not base64.";

    fn file_save(access: Option<&str>) -> PagesParams {
        PagesParams {
            file_path: Some("/Users/me/q3-growth.html".into()),
            access: access.map(Into::into),
            ..params("save")
        }
    }

    #[test]
    fn a_file_save_uploads_to_the_address_of_its_answer() {
        assert_eq!(
            address_to_upload(&file_save(None), SAVE_ANSWER),
            Some(
                "https://api.linkly.ai/v1/uploads/upl_q7Vn3kPx9-2bZ0aLmR4tY8wE1sD6fG5hJ3kL9pO0iU2"
            )
        );
        // New content for a Page that exists, no access asked: it goes up too.
        assert!(address_to_upload(&file_save(None), UPDATE_ANSWER).is_some());
    }

    #[test]
    fn with_access_it_uploads_into_a_new_page_only() {
        assert!(address_to_upload(&file_save(Some("public")), SAVE_ANSWER).is_some());
        // The Page exists, its access stays (public here, private asked): the
        // answer is printed first and nothing goes live.
        assert_eq!(
            address_to_upload(&file_save(Some("private")), UPDATE_ANSWER),
            None
        );
    }

    #[test]
    fn a_save_without_a_file_uploads_nothing() {
        let mut text = params("save");
        text.content = Some("# Q3".into());
        text.content_type = Some("markdown".into());
        assert_eq!(address_to_upload(&text, SAVE_ANSWER), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_path_that_is_not_utf8_is_refused() {
        use std::os::unix::ffi::OsStrExt;
        let path = PathBuf::from(std::ffi::OsStr::from_bytes(b"/home/me/r\xe9sum\xe9/q3.md"));
        assert!(utf8_path(path).is_err_and(|message| message.contains("is not a UTF-8 path")));
        assert!(utf8_path(PathBuf::from("/home/me/résumé/q3.md")).is_ok());
    }
}
