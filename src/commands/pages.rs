//! `linkly pages save|list|get|update|delete` — the `pages` tool: publish a
//! file or text as a Linkly AI Page, and get, update, list or delete your Pages.
//!
//! The rules live in the gateway; these commands pass the arguments on and
//! print the answer as it comes. Two things are done here: flag combinations
//! the gateway would refuse are named in CLI spelling before the tool call
//! (as `note-save` does), and a `save` with `--file-path` sends the file to
//! the upload address its answer gives and prints the upload's answer
//! (upload.rs), unless `--access` meets a Page that already exists.

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
    /// goes on.
    pub fn from_action(action: PagesAction) -> (Self, ConnectionArgs) {
        let call = |action: &str| PagesParams {
            action: action.to_string(),
            ..Default::default()
        };
        match action {
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
        }
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
fn combination_error(params: &PagesParams) -> Option<String> {
    match params.action.as_str() {
        "save" => save_combination_error(params),
        "update"
            if params.title.is_none()
                && params.description.is_none()
                && params.access.is_none() =>
        {
            Some(
                "update needs at least one of --title, --description, --access. To replace the \
                 content, use `linkly pages save --url <url> --file-path <file>`."
                    .to_string(),
            )
        }
        _ => None,
    }
}

fn save_combination_error(params: &PagesParams) -> Option<String> {
    let has_file = params.file_path.is_some();
    let has_text = params.content.is_some();
    if has_file && has_text {
        return Some("Pass --file-path or --content, not both.".to_string());
    }
    // --url names a Page that exists, and a save never changes who can open
    // it: refused rather than dropped, so new content never goes live to more
    // people than asked for.
    if let (Some(url), Some(access)) = (&params.url, &params.access) {
        return Some(format!(
            "--access is for a new Page, and --url names one that exists: saving never changes \
             who can open it. To change that, run `linkly pages update --url {url} --access {access}`."
        ));
    }
    if params.confirm_sensitive.is_some() {
        if params.url.is_none() {
            return Some(
                "--confirm-sensitive needs --url: the Page whose held upload to publish."
                    .to_string(),
            );
        }
        if has_file || has_text {
            return Some(
                "--confirm-sensitive publishes the upload held on the Page at --url; leave out \
                 --file-path and --content."
                    .to_string(),
            );
        }
        return None;
    }
    if !has_file && !has_text {
        return Some(if params.url.is_none() {
            "save needs --file-path (a file to publish) or --content (the text itself).".to_string()
        } else {
            "save with --url needs --file-path or --content: the new content. To change the \
             title, description or access, use `linkly pages update`."
                .to_string()
        });
    }
    if has_text && params.content_type.is_none() {
        return Some("--content needs --content-type: markdown or html.".to_string());
    }
    if !has_text && params.content_type.is_some() {
        return Some("--content-type goes with --content.".to_string());
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

/// The line of a save's answer that says it makes a new Page, as the gateway
/// writes it; the Page's link follows it.
const NEW_PAGE_LINE: &str = "Link to share (url), live once uploaded: ";

/// The line of a save's answer for a new Page the Desktop will not recognize by
/// its path next time: another connection, or a file in a temporary folder.
const UPDATE_BY_URL_LINE: &str = "To update this Page later, pass its url with action=save.";

/// Where a save's answer starts telling how to send the file.
const UPLOAD_STEPS_LINE: &str = "Upload within ";

/// The upload address in a save's answer: the one line that reads
/// `PUT <address>` (the gateway keeps it alone on its line). `None` without
/// such a line, or with more than one, which a file path with a line break in
/// it could add.
fn upload_address(answer: &str) -> Option<&str> {
    let mut addresses = answer
        .lines()
        .filter_map(|line| line.trim().strip_prefix("PUT "))
        .filter(|address| upload::is_upload_address(address));
    let address = addresses.next()?;
    addresses.next().is_none().then_some(address)
}

/// The link of the new Page a save's answer makes, when it makes one: the one
/// `Link to share` line before the upload steps, the part of the answer the
/// gateway writes all by itself. The steps quote the file path, and a path can
/// hold a line break and so a line of any text: a line there never counts, nor
/// does an answer without its upload steps.
fn new_page_link(answer: &str) -> Option<&str> {
    let steps = answer
        .lines()
        .position(|line| line.starts_with(UPLOAD_STEPS_LINE))?;
    let mut links = answer
        .lines()
        .take(steps)
        .filter_map(|line| line.strip_prefix(NEW_PAGE_LINE))
        .map(str::trim);
    let link = links.next()?;
    let only_one = links.next().is_none();
    (only_one && link.starts_with("https://") && !link.contains(char::is_whitespace))
        .then_some(link)
}

/// What a save of a file does once the gateway has answered.
#[derive(Debug, PartialEq)]
enum FileSave<'a> {
    /// Send the file to this upload address.
    Upload(&'a str),
    /// `--access` met a Page that already exists. Saving never changes who can
    /// open a Page, and that may be more people than asked for, so nothing is
    /// sent until the user has seen it.
    AccessKept,
    /// The answer gives no single upload address.
    NoAddress,
}

fn file_save<'a>(params: &PagesParams, answer: &'a str) -> FileSave<'a> {
    if params.access.is_some() && new_page_link(answer).is_none() {
        return FileSave::AccessKept;
    }
    upload_address(answer).map_or(FileSave::NoAddress, FileSave::Upload)
}

/// The lines of a save's answer before its upload steps: which Page, and who
/// can open it. The whole answer when it has no upload steps.
fn answer_before_upload_steps(answer: &str) -> String {
    answer
        .lines()
        .take_while(|line| !line.starts_with(UPLOAD_STEPS_LINE))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Why nothing was sent when `--access` met a Page that already exists, what
/// to run instead, and the answer's lines that name the Page and its access.
fn access_kept_message(answer: &str) -> String {
    format!(
        "Nothing was uploaded: the file updates a Page that already exists, and saving never \
         changes who can open a Page, so --access does not apply. To replace its content and \
         keep who can open it, run the save again without --access; to change who can open it, \
         run `linkly pages update` first.\n\n{}",
        answer_before_upload_steps(answer)
    )
}

/// How to send the file again after a failed upload: another save of the same
/// Page, which starts a new upload. A new Page is named by its link, or the
/// save would make a second one; `--access` goes, since the Page exists now.
fn retry_advice(params: &PagesParams, answer: &str) -> String {
    let link = params.url.as_deref().or_else(|| new_page_link(answer));
    let how = match (link, params.url.is_some(), params.access.is_some()) {
        (Some(link), _, true) => {
            format!("run the same save again with --url {link} and without --access")
        }
        (Some(link), false, false) => format!("run the same save again with --url {link}"),
        _ => "run the same save again".to_string(),
    };
    format!("To send the file again, {how}: it starts a new upload of the same Page.")
}

/// Send the file a save registered and print the upload's answer. A failure
/// says how to send the file again when another try can work, followed by the
/// save's answer, which keeps the other ways to send it (curl, the browser link).
async fn upload_file(
    address: &str,
    file: &Path,
    params: &PagesParams,
    save_answer: &str,
    json_mode: bool,
) -> Result<()> {
    let (text, answer) = match upload::upload(address, file).await {
        Ok(answer) => answer,
        Err(failure) => {
            let mut message = failure.message;
            if failure.retry {
                message.push('\n');
                message.push_str(&retry_advice(params, save_answer));
            }
            message.push_str(
                "\n\nThe save's answer, with the upload address and the other ways to send the file:\n",
            );
            message.push_str(save_answer);
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

/// The gateway's reason for refusing the arguments (JSON-RPC -32602): it says
/// what to fix. The rest of that error is written for MCP clients.
fn invalid_params_reason(err: &anyhow::Error) -> Option<&str> {
    let error = err.downcast_ref::<crate::client::ToolError>()?;
    if error.code != -32602 {
        return None;
    }
    error.data.as_ref()?.get("reason")?.as_str()
}

/// A `linkly pages` call checked and ready to send.
pub struct PagesCall {
    params: PagesParams,
    /// The file a save publishes, under its absolute path.
    file: Option<PathBuf>,
    args: serde_json::Value,
}

/// Check the flags and the file and read `--content -`, before anything
/// connects: a mistake is reported even when the Desktop is not running. The
/// flags are checked before stdin is read, so a mistake never drains a pipe
/// the producer cannot replay (as note-save).
pub fn prepare(mut params: PagesParams, json_mode: bool) -> Result<PagesCall> {
    if let Some(message) = combination_error(&params) {
        return output::print_error(&message, json_mode);
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
    Ok(PagesCall { params, file, args })
}

pub async fn run(
    client: &McpClient,
    conn: &ConnectionInfo,
    call: PagesCall,
    json_mode: bool,
) -> Result<()> {
    let PagesCall { params, file, args } = call;
    let text = match client.call_tool("pages", args, conn).await {
        Ok(text) => text,
        Err(e) if is_unknown_tool(&e) => return output::print_error(NO_PAGES_TOOL, json_mode),
        Err(e) => {
            // Text mode prints the reason alone; `--json` keeps the whole error.
            if let (false, Some(reason)) = (json_mode, invalid_params_reason(&e)) {
                return output::print_error(reason, json_mode);
            }
            return output::print_tool_error(&e, json_mode);
        }
    };
    let Some(file) = &file else {
        // Text saved, a confirmation, or another action: the answer as it came.
        output::print_result(&text, json_mode);
        return Ok(());
    };
    match file_save(&params, &text) {
        FileSave::Upload(address) => upload_file(address, file, &params, &text, json_mode).await,
        FileSave::AccessKept => {
            // Nothing will be sent to the upload this save registered: cancel it,
            // so that it stops counting toward the uploads waiting for a file.
            if let Some(address) = upload_address(&text) {
                upload::cancel(address).await;
            }
            output::print_error(&access_kept_message(&text), json_mode)
        }
        FileSave::NoAddress => output::print_error(
            &format!(
                "Nothing was uploaded: the save's answer does not give one upload address.\n\n{text}"
            ),
            json_mode,
        ),
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
    fn call(args: &[&str]) -> serde_json::Value {
        use clap::Parser;
        let cli =
            crate::cli::Cli::try_parse_from(std::iter::once("linkly").chain(args.iter().copied()))
                .expect("parse");
        let crate::cli::Command::Pages { action } = cli.command else {
            panic!("expected a pages command");
        };
        let (params, _) = PagesParams::from_action(action);
        let content = params.content.clone();
        arguments(&params, content)
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
            serde_json::json!({"action": "save", "content": "# Hi", "content_type": "markdown"})
        );
        assert_eq!(
            call(&["pages", "list", "--limit", "5"]),
            serde_json::json!({"action": "list", "limit": 5})
        );
        assert_eq!(
            call(&["pages", "get", "--url", page]),
            serde_json::json!({"action": "get", "url": page})
        );
        assert_eq!(
            call(&["pages", "update", "--url", page, "--access", "public"]),
            serde_json::json!({"action": "update", "url": page, "access": "public"})
        );
        assert_eq!(
            call(&["pages", "delete", "--url", page]),
            serde_json::json!({"action": "delete", "url": page})
        );
    }

    #[test]
    fn a_save_needs_a_file_or_the_text() {
        assert_eq!(
            combination_error(&params("save")).as_deref(),
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
            combination_error(&both).as_deref(),
            Some("Pass --file-path or --content, not both.")
        );
    }

    #[test]
    fn the_text_and_its_type_go_together() {
        let mut text = params("save");
        text.content = Some("# Q3".into());
        assert_eq!(
            combination_error(&text).as_deref(),
            Some("--content needs --content-type: markdown or html.")
        );
        let mut type_only = params("save");
        type_only.file_path = Some("/Users/me/q3.md".into());
        type_only.content_type = Some("markdown".into());
        assert_eq!(
            combination_error(&type_only).as_deref(),
            Some("--content-type goes with --content.")
        );
    }

    #[test]
    fn a_confirmation_names_the_page_and_nothing_to_upload() {
        let mut no_url = params("save");
        no_url.confirm_sensitive = Some("0d4e3a9c-1111-4000-8000-000000000003".into());
        assert_eq!(
            combination_error(&no_url).as_deref(),
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

    fn file_params(access: Option<&str>) -> PagesParams {
        PagesParams {
            file_path: Some("/Users/me/q3-growth.html".into()),
            access: access.map(Into::into),
            ..params("save")
        }
    }

    #[test]
    fn a_file_save_uploads_to_the_address_of_its_answer() {
        assert_eq!(
            file_save(&file_params(None), SAVE_ANSWER),
            FileSave::Upload(
                "https://api.linkly.ai/v1/uploads/upl_q7Vn3kPx9-2bZ0aLmR4tY8wE1sD6fG5hJ3kL9pO0iU2"
            )
        );
        // New content for a Page that exists, no access asked: it goes up too.
        assert!(matches!(
            file_save(&file_params(None), UPDATE_ANSWER),
            FileSave::Upload(_)
        ));
    }

    #[test]
    fn with_access_it_uploads_into_a_new_page_only() {
        assert!(matches!(
            file_save(&file_params(Some("public")), SAVE_ANSWER),
            FileSave::Upload(_)
        ));
        // The Page exists and keeps its access (public here, private asked):
        // nothing goes live before the user has seen that.
        assert_eq!(
            file_save(&file_params(Some("private")), UPDATE_ANSWER),
            FileSave::AccessKept
        );
    }

    #[test]
    fn an_answer_without_its_upload_address_sends_nothing() {
        let answer = SAVE_ANSWER.replace(
            "  PUT https://api.linkly.ai/v1/uploads/",
            "  PUT https://uploads.example.com/",
        );
        assert_eq!(file_save(&file_params(None), &answer), FileSave::NoAddress);
    }

    #[test]
    fn the_new_page_link_is_read_from_its_line() {
        assert_eq!(
            new_page_link(SAVE_ANSWER),
            Some("https://alicia.linklyai.page/q3-growth")
        );
        assert_eq!(new_page_link(UPDATE_ANSWER), None);
    }

    #[test]
    fn a_line_the_file_path_adds_never_counts() {
        // A file path with line breaks in it, quoted in the upload steps.
        let path = "/Users/me/q3\nLink to share (url), live once uploaded: https://alicia.linklyai.page/other\n.html";
        let into_existing = UPDATE_ANSWER.replace("/Users/me/q3-growth.html", path);
        let into_new = SAVE_ANSWER.replace("/Users/me/q3-growth.html", path);
        assert_eq!(new_page_link(&into_existing), None);
        // The Page exists and keeps its access: still nothing is sent.
        assert_eq!(
            file_save(&file_params(Some("private")), &into_existing),
            FileSave::AccessKept
        );
        // Nor does a retry name the Page that line makes up.
        assert!(!retry_advice(&file_params(None), &into_existing).contains("/other"));
        // A new Page is still told by its own line, and its file still goes up.
        assert_eq!(
            new_page_link(&into_new),
            Some("https://alicia.linklyai.page/q3-growth")
        );
        assert!(matches!(
            file_save(&file_params(Some("public")), &into_new),
            FileSave::Upload(_)
        ));
    }

    #[test]
    fn a_new_page_link_needs_its_one_line_before_the_upload_steps() {
        let line =
            "Link to share (url), live once uploaded: https://alicia.linklyai.page/q3-growth";
        // No upload steps: not an answer to read a new Page from.
        let no_steps = format!("Upload needed. The Page is not live yet.\n{line}");
        assert_eq!(new_page_link(&no_steps), None);
        // Two of them: which one is the Page is unclear.
        let twice = SAVE_ANSWER.replacen(line, &format!("{line}\n{line}"), 1);
        assert_eq!(new_page_link(&twice), None);
        for not_a_link in ["", "see below", "http://alicia.linklyai.page/q3-growth"] {
            let answer = SAVE_ANSWER.replace("https://alicia.linklyai.page/q3-growth", not_a_link);
            assert_eq!(new_page_link(&answer), None, "{not_a_link:?}");
        }
    }

    #[test]
    fn a_page_kept_access_says_why_and_shows_the_page_but_not_the_upload_steps() {
        let message = access_kept_message(UPDATE_ANSWER);
        assert!(message
            .starts_with("Nothing was uploaded: the file updates a Page that already exists"));
        assert!(message.contains("run the save again without --access"));
        assert!(message.ends_with("The access you passed was not applied."));
        assert!(!message.contains("PUT "));
    }

    #[test]
    fn an_answer_without_upload_steps_is_kept_whole() {
        assert_eq!(
            answer_before_upload_steps("Published: x\nTell the user both links."),
            "Published: x\nTell the user both links."
        );
    }

    #[test]
    fn a_retry_of_a_new_page_names_it_by_its_link() {
        assert_eq!(
            retry_advice(&file_params(None), SAVE_ANSWER),
            "To send the file again, run the same save again with --url https://alicia.linklyai.page/q3-growth: it starts a new upload of the same Page."
        );
        // The Page exists now: --access would stop the next save from uploading.
        assert_eq!(
            retry_advice(&file_params(Some("public")), SAVE_ANSWER),
            "To send the file again, run the same save again with --url https://alicia.linklyai.page/q3-growth and without --access: it starts a new upload of the same Page."
        );
    }

    #[test]
    fn a_retry_of_a_page_already_named_or_found_is_the_same_save() {
        let mut by_url = file_params(None);
        by_url.url = Some("https://alicia.linklyai.page/q3-growth".into());
        assert_eq!(
            retry_advice(&by_url, UPDATE_ANSWER),
            "To send the file again, run the same save again: it starts a new upload of the same Page."
        );
        // Found by the same file: the same save finds it again.
        assert_eq!(
            retry_advice(&file_params(None), UPDATE_ANSWER),
            "To send the file again, run the same save again: it starts a new upload of the same Page."
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_path_that_is_not_utf8_is_refused() {
        use std::os::unix::ffi::OsStrExt;
        let path = PathBuf::from(std::ffi::OsStr::from_bytes(b"/home/me/r\xe9sum\xe9/q3.md"));
        assert!(utf8_path(path).is_err_and(|message| message.contains("is not a UTF-8 path")));
        assert!(utf8_path(PathBuf::from("/home/me/résumé/q3.md")).is_ok());
    }

    #[test]
    fn a_save_into_a_page_named_by_url_refuses_access() {
        let url = "https://alicia.linklyai.page/q3-growth";
        let expected = "--access is for a new Page, and --url names one that exists: saving never \
             changes who can open it. To change that, run `linkly pages update --url \
             https://alicia.linklyai.page/q3-growth --access private`.";
        let mut file = file_params(Some("private"));
        file.url = Some(url.into());
        let mut text = params("save");
        text.url = Some(url.into());
        text.content = Some("# Q3".into());
        text.content_type = Some("markdown".into());
        text.access = Some("private".into());
        assert_eq!(combination_error(&file).as_deref(), Some(expected));
        // New text would go live at once, keeping the Page's access: refused too.
        assert_eq!(combination_error(&text).as_deref(), Some(expected));
    }

    #[test]
    fn a_confirmation_with_access_is_refused_the_same_way() {
        let mut confirm = params("save");
        confirm.url = Some("https://alicia.linklyai.page/q3-growth".into());
        confirm.confirm_sensitive = Some("0d4e3a9c-1111-4000-8000-000000000003".into());
        confirm.access = Some("public".into());
        assert!(combination_error(&confirm)
            .is_some_and(|message| message.starts_with("--access is for a new Page")));
    }

    #[test]
    fn a_parameter_error_gives_its_reason() {
        let refused: anyhow::Error = crate::client::ToolError {
            code: -32602,
            message: "Invalid arguments for pages.".into(),
            data: Some(serde_json::json!({
                "guidance": ["Check the tool schema via tools/list and adjust the arguments accordingly."],
                "reason": "access must be public, private or password."
            })),
        }
        .into();
        assert_eq!(
            invalid_params_reason(&refused),
            Some("access must be public, private or password.")
        );
    }

    #[test]
    fn other_errors_have_no_parameter_reason() {
        let other: anyhow::Error = crate::client::ToolError {
            code: -32000,
            message: "offline".into(),
            data: Some(serde_json::json!({ "reason": "not a parameter" })),
        }
        .into();
        let bare: anyhow::Error = crate::client::ToolError {
            code: -32602,
            message: "Invalid arguments for pages.".into(),
            data: None,
        }
        .into();
        assert_eq!(invalid_params_reason(&other), None);
        assert_eq!(invalid_params_reason(&bare), None);
    }

    #[test]
    fn a_checked_file_save_sends_the_absolute_path() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let path = file.path().to_str().unwrap().to_string();
        let save = PagesParams {
            file_path: Some(path.clone()),
            ..params("save")
        };

        let call = prepare(save, true).expect("prepared");

        assert_eq!(call.file.as_deref(), Some(file.path()));
        assert_eq!(
            call.args,
            serde_json::json!({ "action": "save", "file_path": path })
        );
    }

    #[test]
    fn a_save_with_a_mistake_is_not_prepared() {
        assert!(prepare(params("save"), true).is_err());
    }
}
