//! `linkly auth login` — sign in through the browser
//! (LinklyAI/linkly-ai-workspace#177).
//!
//! The CLI starts a login on the website and gets a one-time link plus a
//! confirmation code; the user opens the link on any device and approves; the
//! CLI polls until the website hands over a freshly issued `lkai_` API key,
//! which is stored exactly like one saved with `set-key` (plus a `source`
//! marker, so logout knows it may revoke it).
//!
//! Agents split the flow in two so the link reaches the user before anything
//! blocks: `--no-wait` starts and returns the link, `--wait` (repeatable)
//! finishes. The state in between lives in `pending-login.json` next to the
//! credentials. One config directory holds one account, so a second login is
//! refused while one is pending or a key is already stored.

use std::io::{IsTerminal, Write};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use owo_colors::OwoColorize;
use serde::{Deserialize, Serialize};

use crate::commands::auth::mask_key;
use crate::connection::{
    config_dir, credentials_path, read_credentials_api_key, save_login_api_key, REMOTE_BASE_URL,
};
use crate::constants::web_base_url;

const PENDING_FILE: &str = "pending-login.json";
/// Default limit for one `--wait`; the plain interactive login waits until
/// the link expires instead.
pub const DEFAULT_WAIT_TIMEOUT_SECS: u64 = 300;
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
/// Consecutive network failures tolerated while polling before giving up.
const MAX_NETWORK_FAILURES: u32 = 3;

/// How `login` was invoked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginMode {
    /// Start, show the link, wait for approval.
    StartAndWait,
    /// Start, return the link immediately (`--no-wait`).
    StartOnly,
    /// Resume waiting on the pending login (`--wait`).
    WaitOnly,
}

pub struct LoginOptions {
    pub mode: LoginMode,
    /// Upper bound for this invocation's waiting, in seconds.
    pub timeout: Option<u64>,
    pub restart: bool,
    pub no_browser: bool,
    pub json: bool,
}

/// Everything `--wait` needs to pick up where `--no-wait` left off.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct PendingLogin {
    device_code: String,
    user_code: String,
    url: String,
    /// Unix seconds.
    expires_at: u64,
    /// Seconds between polls; raised when the server answers slow_down.
    interval: u64,
    /// Website the login was started on, so the same one is polled.
    web_base: String,
}

/// The failure codes agents branch on. Every message names the next command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginErrorCode {
    Timeout,
    Expired,
    Denied,
    NoPendingLogin,
    LoginInProgress,
    AlreadyLoggedIn,
    Network,
}

impl LoginErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::Expired => "expired",
            Self::Denied => "denied",
            Self::NoPendingLogin => "no_pending_login",
            Self::LoginInProgress => "login_in_progress",
            Self::AlreadyLoggedIn => "already_logged_in",
            Self::Network => "network",
        }
    }
}

#[derive(Debug)]
pub struct LoginError {
    pub code: LoginErrorCode,
    pub message: String,
}

impl std::fmt::Display for LoginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for LoginError {}

fn fail(code: LoginErrorCode, message: impl Into<String>) -> anyhow::Error {
    LoginError {
        code,
        message: message.into(),
    }
    .into()
}

/// Entry point: runs the login and reports any failure in the chosen format.
pub async fn run(opts: LoginOptions) -> Result<()> {
    match execute(&opts).await {
        Ok(()) => Ok(()),
        Err(e) => {
            let (code, message) = match e.downcast_ref::<LoginError>() {
                Some(le) => (Some(le.code.as_str()), le.message.clone()),
                None => (None, format!("{:#}", e)),
            };
            if opts.json {
                let mut obj = serde_json::json!({ "status": "error", "message": message });
                if let Some(code) = code {
                    obj["code"] = serde_json::json!(code);
                }
                println!("{}", obj);
            } else {
                eprintln!("Error: {}", message);
            }
            Err(anyhow::Error::msg(""))
        }
    }
}

async fn execute(opts: &LoginOptions) -> Result<()> {
    let pending_path = config_dir()?.join(PENDING_FILE);

    if opts.mode == LoginMode::WaitOnly {
        let Some(pending) = load_pending(&pending_path)? else {
            return Err(fail(
                LoginErrorCode::NoPendingLogin,
                "No login is waiting for approval. Run `linkly auth login --no-wait` to start one.",
            ));
        };
        if pending.expires_at <= unix_now() {
            remove_pending(&pending_path)?;
            return Err(expired());
        }
        let timeout = opts.timeout.unwrap_or(DEFAULT_WAIT_TIMEOUT_SECS);
        return wait_for_approval(pending, &pending_path, timeout, opts.json).await;
    }

    if read_credentials_api_key()?.is_some() {
        return Err(fail(
            LoginErrorCode::AlreadyLoggedIn,
            format!(
                "Already logged in ({}). Run `linkly auth logout` first, or set LINKLY_CONFIG_DIR \
                 to sign in another account in a separate directory.",
                credentials_path()?.display()
            ),
        ));
    }

    let existing = load_pending(&pending_path)?;
    if opts.restart
        || existing
            .as_ref()
            .is_some_and(|p| p.expires_at <= unix_now())
    {
        remove_pending(&pending_path)?;
    } else if let Some(pending) = existing {
        return Err(fail(
            LoginErrorCode::LoginInProgress,
            format!(
                "A login is already waiting for approval (code {}, link {}, {}s left). \
                 Run `linkly auth login --wait` to finish it, or `linkly auth login --restart` to start over.",
                pending.user_code,
                pending.url,
                pending.expires_at.saturating_sub(unix_now())
            ),
        ));
    }

    let pending = start_login().await?;
    create_pending(&pending_path, &pending)?;

    match opts.mode {
        LoginMode::StartOnly => {
            let expires_in = pending.expires_at.saturating_sub(unix_now());
            if opts.json {
                println!(
                    "{}",
                    serde_json::json!({
                        "status": "pending",
                        "url": pending.url,
                        "user_code": pending.user_code,
                        "expires_in": expires_in,
                    })
                );
            } else {
                print_link(&mut std::io::stdout(), &pending, false);
                println!(
                    "\nThe link is valid for {} minutes. After approving, run `linkly auth login --wait`.",
                    expires_in / 60
                );
            }
            Ok(())
        }
        _ => {
            let opened = !opts.no_browser && can_open_browser() && open_browser(&pending.url);
            // In JSON mode stdout carries exactly one object (the result), so
            // the link goes to stderr where a person still sees it.
            if opts.json {
                print_link(&mut std::io::stderr(), &pending, opened);
            } else {
                print_link(&mut std::io::stdout(), &pending, opened);
                println!("Waiting for authorization… (Ctrl+C to stop; `linkly auth login --wait` resumes)");
                let _ = std::io::stdout().flush();
            }
            let until_expiry = pending.expires_at.saturating_sub(unix_now());
            let timeout = opts.timeout.unwrap_or(until_expiry);
            wait_for_approval(pending, &pending_path, timeout, opts.json).await
        }
    }
}

fn print_link(out: &mut dyn Write, pending: &PendingLogin, opened: bool) {
    let _ = writeln!(
        out,
        "Open this link in a browser to authorize the CLI{}:\n  {}\nConfirmation code: {} (check that the page shows the same code)",
        if opened { " (opened in your browser)" } else { "" },
        pending.url,
        pending.user_code
    );
    let _ = out.flush();
}

// ─── server calls ───────────────────────────────────────────────

#[derive(Deserialize)]
struct StartResponse {
    device_code: String,
    user_code: String,
    verification_uri_complete: String,
    expires_in: u64,
    interval: u64,
}

/// Server error body shape shared by the /api/cli/auth routes.
#[derive(Deserialize)]
struct ServerError {
    message: Option<String>,
}

fn http_client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder().timeout(HTTP_TIMEOUT).build()?)
}

async fn start_login() -> Result<PendingLogin> {
    let web_base = web_base_url();
    let body = serde_json::json!({
        "client_type": "cli",
        "client_info": {
            "hostname": hostname(),
            "os": std::env::consts::OS,
            "version": env!("CARGO_PKG_VERSION"),
        },
    });
    let network = |detail: String| {
        fail(
            LoginErrorCode::Network,
            format!("Could not start the login at {}: {}. Check your connection and run `linkly auth login` again.", web_base, detail),
        )
    };
    let resp = http_client()?
        .post(format!("{}/api/cli/auth/start", web_base))
        .json(&body)
        .send()
        .await
        .map_err(|e| network(e.without_url().to_string()))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let detail = resp
            .json::<ServerError>()
            .await
            .ok()
            .and_then(|e| e.message)
            .unwrap_or_else(|| format!("HTTP {}", status));
        return Err(network(detail));
    }
    let started: StartResponse = resp
        .json()
        .await
        .map_err(|_| network("unexpected response".to_string()))?;
    Ok(PendingLogin {
        device_code: started.device_code,
        user_code: started.user_code,
        url: started.verification_uri_complete,
        expires_at: unix_now() + started.expires_in,
        interval: started.interval.max(1),
        web_base,
    })
}

/// One poll's answer. Unknown statuses are treated like `pending` by the
/// caller only if they carry an interval; otherwise they surface as errors.
#[derive(Deserialize)]
struct PollResponse {
    status: String,
    interval: Option<u64>,
    api_key: Option<String>,
}

enum PollOutcome {
    Pending { interval: Option<u64> },
    SlowDown { interval: Option<u64> },
    Approved { api_key: String },
    Denied,
    Gone,
}

async fn poll_once(pending: &PendingLogin) -> std::result::Result<PollOutcome, String> {
    let resp = http_client()
        .map_err(|e| e.to_string())?
        .post(format!("{}/api/cli/auth/poll", pending.web_base))
        .json(&serde_json::json!({ "device_code": pending.device_code }))
        .send()
        .await
        .map_err(|e| e.without_url().to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let body: PollResponse = resp
        .json()
        .await
        .map_err(|_| "unexpected response".to_string())?;
    Ok(match body.status.as_str() {
        "pending" => PollOutcome::Pending {
            interval: body.interval,
        },
        "slow_down" => PollOutcome::SlowDown {
            interval: body.interval,
        },
        "approved" => match body.api_key {
            Some(api_key) => PollOutcome::Approved { api_key },
            None => return Err("approved without a key".to_string()),
        },
        "denied" => PollOutcome::Denied,
        "expired" | "invalid" => PollOutcome::Gone,
        other => return Err(format!("unknown status '{}'", other)),
    })
}

// ─── waiting ────────────────────────────────────────────────────

async fn wait_for_approval(
    mut pending: PendingLogin,
    pending_path: &Path,
    timeout_secs: u64,
    json: bool,
) -> Result<()> {
    let wait_deadline = unix_now().saturating_add(timeout_secs);
    let mut network_failures = 0;

    loop {
        let now = unix_now();
        if now >= pending.expires_at {
            remove_pending(pending_path)?;
            return Err(expired());
        }
        // Not enough time left in this wait for another properly spaced
        // poll: stop here rather than poll early and earn a slow_down.
        if now.saturating_add(pending.interval) > wait_deadline {
            tokio::time::sleep(Duration::from_secs(wait_deadline.saturating_sub(now))).await;
            return Err(fail(
                LoginErrorCode::Timeout,
                format!(
                    "Not authorized yet. Run `linkly auth login --wait` again; the link is valid for {} more seconds.",
                    pending.expires_at.saturating_sub(unix_now())
                ),
            ));
        }
        tokio::time::sleep(Duration::from_secs(pending.interval)).await;

        match poll_once(&pending).await {
            Ok(PollOutcome::Pending { interval }) => {
                network_failures = 0;
                if let Some(interval) = interval {
                    pending.interval = interval.max(1);
                }
            }
            Ok(PollOutcome::SlowDown { interval }) => {
                network_failures = 0;
                pending.interval = interval.unwrap_or(pending.interval + 5).max(1);
                // Remembered so the next `--wait` starts at the slower pace.
                let _ = write_pending(pending_path, &pending, false);
            }
            Ok(PollOutcome::Approved { api_key }) => {
                return finish(&api_key, pending_path, &pending, json).await;
            }
            Ok(PollOutcome::Denied) => {
                remove_pending(pending_path)?;
                return Err(fail(
                    LoginErrorCode::Denied,
                    "The login was denied in the browser. Run `linkly auth login` to try again.",
                ));
            }
            Ok(PollOutcome::Gone) => {
                remove_pending(pending_path)?;
                return Err(expired());
            }
            Err(detail) => {
                network_failures += 1;
                if network_failures >= MAX_NETWORK_FAILURES {
                    return Err(fail(
                        LoginErrorCode::Network,
                        format!(
                            "Could not reach {}: {}. The login is still pending; run `linkly auth login --wait` to keep waiting.",
                            pending.web_base, detail
                        ),
                    ));
                }
            }
        }
    }
}

fn expired() -> anyhow::Error {
    fail(
        LoginErrorCode::Expired,
        "The login link has expired. Run `linkly auth login` to get a new one.",
    )
}

/// Store the key, then confirm it with the gateway and report the account.
///
/// The pending file is removed only after the key is safely written: if the
/// write fails, `--wait` can collect the same key again while the link lives.
async fn finish(
    api_key: &str,
    pending_path: &Path,
    pending: &PendingLogin,
    json: bool,
) -> Result<()> {
    save_login_api_key(api_key).with_context(|| {
        format!(
            "Authorized, but the key could not be saved. Fix the problem and run \
             `linkly auth login --wait` within {} seconds to collect the same key",
            pending.expires_at.saturating_sub(unix_now())
        )
    })?;
    remove_pending(pending_path)?;

    let account = validate_key(api_key).await;
    let preview = mask_key(api_key);
    if json {
        let mut obj = serde_json::json!({ "status": "ok", "key_preview": preview });
        match &account {
            Ok(acc) => {
                obj["account"] = serde_json::json!(acc.label());
                obj["plan"] = serde_json::json!(acc.plan);
            }
            Err(reason) => {
                obj["account"] = serde_json::Value::Null;
                obj["validate_error"] = serde_json::json!(reason);
            }
        }
        println!("{}", obj);
    } else {
        match &account {
            Ok(acc) => match &acc.plan {
                Some(plan) => println!("{} Logged in as {} ({})", "✓".green(), acc.label(), display_plan(plan)),
                None => println!("{} Logged in as {}", "✓".green(), acc.label()),
            },
            Err(reason) => println!(
                "{} Logged in. The key could not be verified right now ({}); run `linkly auth status` to check it.",
                "✓".green(),
                reason
            ),
        }
        println!("  Key saved to {}", credentials_path()?.display());
        // Logging in proves the account, not that the desktop or cloud
        // libraries are reachable — say how to check instead of implying it.
        println!("  Run `linkly doctor --remote` to check the remote connection and whether your desktop is online.");
    }
    Ok(())
}

struct Account {
    email: Option<String>,
    username: Option<String>,
    plan: Option<String>,
}

impl Account {
    fn label(&self) -> String {
        self.email
            .clone()
            .or_else(|| self.username.clone())
            .unwrap_or_else(|| "your account".to_string())
    }
}

#[derive(Deserialize)]
struct ValidateResponse {
    email: Option<String>,
    username: Option<String>,
    quota: Option<ValidateQuota>,
}

#[derive(Deserialize)]
struct ValidateQuota {
    plan: Option<String>,
}

/// Ask the gateway who the key belongs to (`/v1/validate`, quota-free).
async fn validate_key(api_key: &str) -> std::result::Result<Account, String> {
    let resp = http_client()
        .map_err(|e| e.to_string())?
        .get(format!("{}/v1/validate", REMOTE_BASE_URL))
        .bearer_auth(api_key)
        .send()
        .await
        .map_err(|_| "gateway unreachable".to_string())?;
    if !resp.status().is_success() {
        return Err(format!("gateway returned HTTP {}", resp.status().as_u16()));
    }
    let body: ValidateResponse = resp
        .json()
        .await
        .map_err(|_| "unexpected gateway response".to_string())?;
    Ok(Account {
        email: body.email,
        username: body.username,
        plan: body.quota.and_then(|q| q.plan),
    })
}

fn display_plan(plan: &str) -> String {
    match plan {
        "pro" => "Pro".to_string(),
        "free" => "Free".to_string(),
        other => other.to_string(),
    }
}

// ─── pending file ───────────────────────────────────────────────

/// The pending login (possibly expired — callers decide), or `None` when there
/// is none. An unreadable file is removed and treated as absent: it can never
/// be completed anyway.
fn load_pending(path: &Path) -> Result<Option<PendingLogin>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(anyhow::anyhow!("Failed to read {}: {}", path.display(), e)),
    };
    match serde_json::from_str::<PendingLogin>(&text) {
        Ok(pending) => Ok(Some(pending)),
        Err(_) => {
            remove_pending(path)?;
            Ok(None)
        }
    }
}

/// Create the pending file, failing if one already exists — the check that
/// keeps two concurrent `login`s in one directory from overwriting each other.
fn create_pending(path: &Path, pending: &PendingLogin) -> Result<()> {
    match write_pending(path, pending, true) {
        Err(e)
            if e.downcast_ref::<std::io::Error>()
                .is_some_and(|io| io.kind() == std::io::ErrorKind::AlreadyExists) =>
        {
            Err(fail(
                LoginErrorCode::LoginInProgress,
                "Another login was started in this directory at the same time. \
                 Run `linkly auth login --wait` to finish it, or `linkly auth login --restart` to start over.",
            ))
        }
        other => other,
    }
}

/// Write atomically (temp file + rename, 0600 on Unix). With `exclusive`, the
/// rename refuses to replace an existing file.
fn write_pending(path: &Path, pending: &PendingLogin, exclusive: bool) -> Result<()> {
    let dir = path.parent().context("Pending login path has no parent")?;
    std::fs::create_dir_all(dir)
        .with_context(|| format!("Failed to create directory: {}", dir.display()))?;
    let mut tmp = tempfile::Builder::new()
        .prefix(".pending-login-")
        .suffix(".tmp")
        .tempfile_in(dir)
        .with_context(|| format!("Failed to create a temp file in {}", dir.display()))?;
    tmp.write_all(serde_json::to_string_pretty(pending)?.as_bytes())?;
    tmp.as_file().sync_all()?;
    if exclusive {
        tmp.persist_noclobber(path)
            .map_err(|e| anyhow::Error::from(e.error))?;
    } else {
        tmp.persist(path)
            .map_err(|e| anyhow::Error::from(e.error))?;
    }
    Ok(())
}

fn remove_pending(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(anyhow::anyhow!(
            "Failed to remove {}: {}",
            path.display(),
            e
        )),
    }
}

// ─── environment ────────────────────────────────────────────────

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Best-effort machine name, shown on the approval page and in the key name.
/// The website sanitizes it; an empty value is fine.
fn hostname() -> String {
    let from_cmd = std::process::Command::new("hostname")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|h| !h.is_empty());
    from_cmd
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_default()
}

/// Whether a browser window would reach the person at this terminal: an
/// interactive session on a machine with a desktop, not an SSH login.
fn can_open_browser() -> bool {
    if !std::io::stdout().is_terminal() || !std::io::stdin().is_terminal() {
        return false;
    }
    if std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_TTY").is_some() {
        return false;
    }
    match std::env::consts::OS {
        "macos" | "windows" => true,
        _ => std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some(),
    }
}

/// Launch the default browser without waiting for it. Returns whether the
/// launcher started; the link is printed either way.
fn open_browser(url: &str) -> bool {
    let mut cmd = match std::env::consts::OS {
        "macos" => {
            let mut c = std::process::Command::new("open");
            c.arg(url);
            c
        }
        "windows" => {
            // `start` is a cmd builtin; the empty string is the window title.
            let mut c = std::process::Command::new("cmd");
            c.args(["/C", "start", "", url]);
            c
        }
        _ => {
            let mut c = std::process::Command::new("xdg-open");
            c.arg(url);
            c
        }
    };
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::{save_credentials_api_key, CONFIG_DIR_ENV};
    use crate::test_helpers::with_temp_home;

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(f)
    }

    fn sample(expires_at: u64) -> PendingLogin {
        PendingLogin {
            device_code: "dc".to_string(),
            user_code: "ABCD-EFGH".to_string(),
            url: "https://linkly.ai/cli/auth?code=ABCD-EFGH".to_string(),
            expires_at,
            interval: 5,
            web_base: "https://linkly.ai".to_string(),
        }
    }

    fn opts(mode: LoginMode) -> LoginOptions {
        LoginOptions {
            mode,
            timeout: Some(1),
            restart: false,
            no_browser: true,
            json: true,
        }
    }

    fn code_of(result: Result<()>) -> LoginErrorCode {
        result
            .expect_err("expected a login error")
            .downcast::<LoginError>()
            .expect("a LoginError")
            .code
    }

    fn pending_path() -> std::path::PathBuf {
        config_dir().unwrap().join(PENDING_FILE)
    }

    #[test]
    fn pending_file_round_trips_and_is_exclusive() {
        with_temp_home("login_pending_exclusive", |_home| {
            let path = pending_path();
            let pending = sample(unix_now() + 600);
            create_pending(&path, &pending).unwrap();
            assert_eq!(load_pending(&path).unwrap(), Some(pending.clone()));

            let err = create_pending(&path, &pending).unwrap_err();
            assert_eq!(
                err.downcast::<LoginError>().unwrap().code,
                LoginErrorCode::LoginInProgress
            );

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
                assert_eq!(mode, 0o600);
            }
        });
    }

    #[test]
    fn corrupt_pending_is_discarded() {
        with_temp_home("login_pending_stale", |_home| {
            let path = pending_path();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "not json").unwrap();
            assert_eq!(load_pending(&path).unwrap(), None);
            assert!(!path.exists());
        });
    }

    #[test]
    fn wait_without_a_pending_login_says_how_to_start() {
        with_temp_home("login_no_pending", |_home| {
            let err = block_on(execute(&opts(LoginMode::WaitOnly))).unwrap_err();
            let le = err.downcast::<LoginError>().unwrap();
            assert_eq!(le.code, LoginErrorCode::NoPendingLogin);
            assert!(le.message.contains("linkly auth login --no-wait"));
        });
    }

    #[test]
    fn wait_on_an_expired_link_reports_expired_and_cleans_up() {
        with_temp_home("login_wait_expired", |_home| {
            create_pending(&pending_path(), &sample(unix_now().saturating_sub(1))).unwrap();
            assert_eq!(
                code_of(block_on(execute(&opts(LoginMode::WaitOnly)))),
                LoginErrorCode::Expired
            );
            assert!(!pending_path().exists());
            // Nothing left to wait on afterwards.
            assert_eq!(
                code_of(block_on(execute(&opts(LoginMode::WaitOnly)))),
                LoginErrorCode::NoPendingLogin
            );
        });
    }

    #[test]
    fn login_refuses_when_a_key_is_already_stored() {
        with_temp_home("login_already", |_home| {
            save_credentials_api_key("lkai_0123456789abcdef0123456789abcdef").unwrap();
            for mode in [LoginMode::StartOnly, LoginMode::StartAndWait] {
                assert_eq!(
                    code_of(block_on(execute(&opts(mode)))),
                    LoginErrorCode::AlreadyLoggedIn
                );
            }
        });
    }

    #[test]
    fn login_refuses_to_overwrite_a_pending_login() {
        with_temp_home("login_in_progress", |_home| {
            create_pending(&pending_path(), &sample(unix_now() + 600)).unwrap();
            let err = block_on(execute(&opts(LoginMode::StartOnly))).unwrap_err();
            let le = err.downcast::<LoginError>().unwrap();
            assert_eq!(le.code, LoginErrorCode::LoginInProgress);
            assert!(le.message.contains("--wait") && le.message.contains("--restart"));
            // The pending login is left untouched.
            assert!(load_pending(&pending_path()).unwrap().is_some());
        });
    }

    #[test]
    fn pending_login_follows_the_config_dir() {
        with_temp_home("login_config_dir", |home| {
            let custom = home.join("tenant-b");
            std::env::set_var(CONFIG_DIR_ENV, &custom);
            let path = pending_path();
            std::env::remove_var(CONFIG_DIR_ENV);
            assert_eq!(path, custom.join(PENDING_FILE));
        });
    }

    #[test]
    fn error_codes_have_stable_names() {
        let names: Vec<&str> = [
            LoginErrorCode::Timeout,
            LoginErrorCode::Expired,
            LoginErrorCode::Denied,
            LoginErrorCode::NoPendingLogin,
            LoginErrorCode::LoginInProgress,
            LoginErrorCode::AlreadyLoggedIn,
            LoginErrorCode::Network,
        ]
        .iter()
        .map(|c| c.as_str())
        .collect();
        assert_eq!(
            names,
            [
                "timeout",
                "expired",
                "denied",
                "no_pending_login",
                "login_in_progress",
                "already_logged_in",
                "network"
            ]
        );
    }
}
