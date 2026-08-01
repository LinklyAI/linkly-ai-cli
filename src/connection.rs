use anyhow::{bail, Context, Result};
use serde::Deserialize;

/// Remote tunnel endpoint
const REMOTE_MCP_URL: &str = "https://mcp.linkly.ai/mcp";
const REMOTE_BASE_URL: &str = "https://mcp.linkly.ai";

/// Credentials file path: ~/.linkly/credentials.json
/// New format: { "remotes": [{ "name": "default", "key": "lkai_xxx" }] }
/// Legacy format (still readable): { "apiKey": "lkai_xxx" }
const CREDENTIALS_FILE: &str = "credentials.json";

/// Remote tunnel health response (from DO /health endpoint).
/// Shared across client.rs, doctor.rs, and status.rs.
#[derive(Deserialize)]
pub struct RemoteHealthResponse {
    #[allow(dead_code)]
    pub status: String,
    pub tunnel: Option<String>,
}

/// Connection mode for error messages and doctor hints.
#[derive(Debug, Clone)]
pub enum ConnectionMode {
    Local,
    Lan { endpoint: String },
    Remote,
}

/// Resolved connection info for the Linkly AI desktop app.
pub struct ConnectionInfo {
    /// MCP endpoint URL (e.g. http://127.0.0.1:60606/mcp)
    pub mcp_url: String,
    /// Base URL (e.g. http://127.0.0.1:60606)
    pub base_url: String,
    /// Optional auth header value (e.g. "Bearer lkai_xxx")
    pub auth_header: Option<String>,
    /// Whether this is a remote tunnel connection (mcp.linkly.ai)
    pub is_remote: bool,
    /// Connection mode for generating contextual error messages
    pub mode: ConnectionMode,
}

impl ConnectionInfo {
    /// Generate a `linkly doctor` command hint matching the current connection mode.
    pub fn doctor_hint(&self) -> String {
        match &self.mode {
            ConnectionMode::Local => {
                "Run 'linkly doctor' to diagnose local connection issues.".to_string()
            }
            ConnectionMode::Lan { endpoint } => {
                format!(
                    "Run 'linkly doctor --endpoint {} --token <your-token>' to diagnose LAN connection issues.",
                    endpoint
                )
            }
            ConnectionMode::Remote => {
                "Run 'linkly doctor --remote' to diagnose remote connection issues.".to_string()
            }
        }
    }
}

/// Resolve the MCP endpoint.
///
/// Three modes:
/// 1. `--endpoint` + `--token` → explicit endpoint (LAN mode, both required by clap)
/// 2. `--remote` → read ~/.linkly/credentials.json, connect to mcp.linkly.ai
///    (`token` is always ignored in this mode; authentication uses credentials file)
/// 3. Default → read ~/.linkly/port (local mode, no auth)
///
/// Note: `endpoint`/`token` and `remote` are mutually exclusive (enforced by clap).
/// When called from `mcp` command, `token` is always `None` and `remote` is `false`.
pub fn resolve(
    endpoint: Option<&str>,
    token: Option<&str>,
    remote: bool,
) -> Result<ConnectionInfo> {
    // Mode 1: explicit endpoint + token for LAN auth
    if let Some(ep) = endpoint {
        let trimmed = ep.trim_end_matches('/');
        let base = trimmed.strip_suffix("/mcp").unwrap_or(trimmed).to_string();
        let mcp = format!("{}/mcp", base);
        let auth_header = token.map(|t| format!("Bearer {}", t));
        let mode = ConnectionMode::Lan {
            endpoint: base.clone(),
        };
        return Ok(ConnectionInfo {
            mcp_url: mcp,
            base_url: base,
            auth_header,
            is_remote: false,
            mode,
        });
    }

    // Mode 2: remote tunnel via mcp.linkly.ai
    if remote {
        let api_key = read_credentials_api_key()?.context(
            "No API key configured for remote mode.\n\n\
                 To set up:\n  \
                   1. Get your API key from https://linkly.ai (Dashboard > API Keys)\n  \
                   2. Run: linkly auth set-key <your-api-key>\n\n\
                 Run 'linkly doctor --remote' to diagnose remote connection issues.",
        )?;

        return Ok(ConnectionInfo {
            mcp_url: REMOTE_MCP_URL.to_string(),
            base_url: REMOTE_BASE_URL.to_string(),
            auth_header: Some(format!("Bearer {}", api_key)),
            is_remote: true,
            mode: ConnectionMode::Remote,
        });
    }

    // Mode 3: local — read ~/.linkly/port
    let port_file = dirs::home_dir()
        .map(|h| h.join(".linkly").join("port"))
        .context("Cannot determine home directory")?;

    let content = std::fs::read_to_string(&port_file).map_err(|_| {
        anyhow::anyhow!(
            "Linkly AI Desktop is not running on this machine.\n\n\
             How to connect:\n  \
               Local:   Launch Linkly AI Desktop on this machine\n  \
               Remote:  linkly <command> --remote  (API key required, get it from https://linkly.ai)\n  \
               LAN:     linkly <command> --endpoint <url> --token <token>  (find token in Desktop > Settings > MCP)\n\n\
             Run 'linkly doctor' to diagnose local connection issues."
        )
    })?;

    const PORT_FILE_CORRUPT: &str = "Linkly AI Desktop port file is corrupted.\n\
         Try restarting the app, or delete ~/.linkly/port and relaunch.\n\n\
         Run 'linkly doctor' to diagnose local connection issues.";

    let parsed: serde_json::Value = serde_json::from_str(&content).context(PORT_FILE_CORRUPT)?;

    let port = parsed["port"].as_u64().context(PORT_FILE_CORRUPT)?;

    if port == 0 || port > 65535 {
        bail!(
            "Invalid port {} in ~/.linkly/port (must be 1-65535).\n\
             Try restarting the app, or delete ~/.linkly/port and relaunch.\n\n\
             Run 'linkly doctor' to diagnose local connection issues.",
            port
        );
    }

    let base = format!("http://127.0.0.1:{}", port);
    let mcp = format!("{}/mcp", base);

    Ok(ConnectionInfo {
        mcp_url: mcp,
        base_url: base,
        auth_header: None,
        is_remote: false,
        mode: ConnectionMode::Local,
    })
}

/// Read API key from ~/.linkly/credentials.json
/// Supports new format (remotes[0].key) with fallback to legacy format (apiKey).
///
/// Returns:
/// - `Ok(Some(key))` — key found and read successfully
/// - `Ok(None)` — file does not exist (user never ran `auth set-key`)
/// - `Err(...)` — file exists but is corrupted or unreadable
pub(crate) fn read_credentials_api_key() -> Result<Option<String>> {
    let path = credentials_path()?;

    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(anyhow::anyhow!(
                "Failed to read credentials file {}: {}",
                path.display(),
                e
            ))
        }
    };

    let parsed: serde_json::Value = serde_json::from_str(&content).with_context(|| {
        format!(
            "Credentials file is corrupted: {}\n\
             Run 'linkly auth set-key <your-api-key>' to fix it.\n\
             Get your API key from https://linkly.ai (Dashboard > API Keys).",
            path.display()
        )
    })?;

    // New format: { "remotes": [{ "name": "default", "key": "lkai_..." }] }
    if let Some(key) = parsed["remotes"]
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|entry| entry["key"].as_str())
    {
        return Ok(Some(key.to_string()));
    }

    // Legacy format: { "apiKey": "lkai_..." }
    Ok(parsed["apiKey"].as_str().map(|s| s.to_string()))
}

/// Explain a failed local/LAN connection that an environment proxy is silently
/// intercepting.
///
/// `reqwest` honours `http_proxy` / `all_proxy` like curl and Python do, and
/// like them it does **not** exempt loopback. So with a proxy configured and no
/// matching `no_proxy` entry, a request to `127.0.0.1` leaves the machine — and
/// when the desktop app isn't running, the error the user sees is whatever the
/// proxy says (typically `HTTP 502`) rather than "nothing is listening".
///
/// We deliberately do not change that behaviour: the proxy variables are the
/// user's explicit configuration, and the same gap affects every other tool on
/// the machine. What we can do is stop the resulting error from pointing in the
/// wrong direction.
///
/// Returns `None` when nothing would be intercepted — remote connections (where
/// using the proxy is the point), no proxy configured, or a `no_proxy` that
/// already covers the host.
pub fn proxy_interception_note(conn: &ConnectionInfo) -> Option<String> {
    if conn.is_remote {
        return None;
    }
    let proxy = first_env(&["http_proxy", "HTTP_PROXY", "all_proxy", "ALL_PROXY"])?;
    let host = host_of(&conn.base_url)?;
    let no_proxy = first_env(&["no_proxy", "NO_PROXY"]).unwrap_or_default();
    if no_proxy_covers(&no_proxy, &host) {
        return None;
    }
    // First line stands alone as a complete sentence: `doctor` shows only that
    // line inside its check table, where the multi-line form doesn't fit.
    Some(format!(
        "Note: a proxy ({proxy}) is intercepting local requests — no_proxy does not cover {host}.\n\
         An HTTP error above may be the proxy's answer rather than Linkly's. To rule it out:\n\
         \n\
         \x20   no_proxy={host},localhost linkly <command>"
    ))
}

/// First of `names` that is set to a non-empty value.
fn first_env(names: &[&str]) -> Option<String> {
    names
        .iter()
        .filter_map(|n| std::env::var(n).ok())
        .find(|v| !v.trim().is_empty())
}

/// Host portion of a `scheme://host:port` URL.
fn host_of(url: &str) -> Option<String> {
    let rest = url.split("://").nth(1).unwrap_or(url);
    let host = rest.split('/').next()?;
    let host = match host.rsplit_once(':') {
        // Keep bracketed IPv6 literals intact; only strip a trailing port.
        Some((h, port)) if port.chars().all(|c| c.is_ascii_digit()) => h,
        _ => host,
    };
    let host = host.trim_matches(|c| c == '[' || c == ']');
    (!host.is_empty()).then(|| host.to_string())
}

/// Does `no_proxy` exempt `host`?
///
/// A pragmatic subset of the (unstandardised) no_proxy syntax: `*` matches
/// everything, an entry matches the host exactly, and a leading-dot entry
/// matches by domain suffix. CIDR notation is not parsed — `127.0.0.0/8` is
/// recognised only through the loopback shorthand below.
fn no_proxy_covers(no_proxy: &str, host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    for entry in no_proxy.split(',') {
        let entry = entry.trim().trim_end_matches('.').to_ascii_lowercase();
        if entry.is_empty() {
            continue;
        }
        if entry == "*" || entry == host {
            return true;
        }
        if let Some(suffix) = entry.strip_prefix('.') {
            if host.ends_with(suffix) {
                return true;
            }
        }
        // A loopback entry covers the other spellings of the same machine —
        // users write one of them and mean all of them.
        if is_loopback(&entry) && is_loopback(&host) {
            return true;
        }
    }
    false
}

fn is_loopback(host: &str) -> bool {
    host == "localhost" || host == "::1" || host.starts_with("127.")
}

/// Absolute path of the credentials file (`~/.linkly/credentials.json`).
///
/// Single source of truth so read / write / delete can never disagree about
/// where the file lives — and so `auth status` can show the user the real path
/// instead of a hardcoded string.
pub fn credentials_path() -> Result<std::path::PathBuf> {
    Ok(dirs::home_dir()
        .context("Cannot determine home directory")?
        .join(".linkly")
        .join(CREDENTIALS_FILE))
}

/// Delete the credentials file.
///
/// Returns whether a file was actually removed. A missing file is **not** an
/// error: the caller asked for "no stored credentials", and that already holds.
pub fn delete_credentials() -> Result<bool> {
    let path = credentials_path()?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(anyhow::anyhow!(
            "Failed to remove credentials file {}: {}",
            path.display(),
            e
        )),
    }
}

/// Save API key to ~/.linkly/credentials.json (new format with remotes array)
pub fn save_credentials_api_key(api_key: &str) -> Result<()> {
    let path = credentials_path()?;
    let dir = path
        .parent()
        .context("Credentials path has no parent directory")?;

    std::fs::create_dir_all(dir)
        .with_context(|| format!("Failed to create directory: {}", dir.display()))?;

    let content = serde_json::json!({
        "remotes": [{ "name": "default", "key": api_key }]
    });
    let json_bytes = serde_json::to_string_pretty(&content)?;

    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)
            .with_context(|| format!("Failed to open credentials file: {}", path.display()))?;
        file.write_all(json_bytes.as_bytes())
            .with_context(|| format!("Failed to write credentials: {}", path.display()))?;
    }

    #[cfg(not(unix))]
    {
        std::fs::write(&path, json_bytes)
            .with_context(|| format!("Failed to write credentials: {}", path.display()))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::with_temp_home;

    // ── no_proxy matching ────────────────────────────────

    #[test]
    fn wildcard_no_proxy_covers_everything() {
        assert!(no_proxy_covers("*", "127.0.0.1"));
        assert!(no_proxy_covers("*", "192.168.1.5"));
    }

    #[test]
    fn exact_host_entry_matches() {
        assert!(no_proxy_covers("127.0.0.1", "127.0.0.1"));
        assert!(no_proxy_covers("example.com,127.0.0.1", "127.0.0.1"));
        assert!(!no_proxy_covers("example.com", "127.0.0.1"));
    }

    #[test]
    fn loopback_spellings_are_interchangeable() {
        // Users write one spelling and mean the machine, not that literal string.
        assert!(no_proxy_covers("localhost", "127.0.0.1"));
        assert!(no_proxy_covers("127.0.0.1", "localhost"));
        assert!(no_proxy_covers("127.0.0.1", "127.0.0.53"));
        // But a loopback entry must not exempt a LAN host.
        assert!(!no_proxy_covers("localhost", "192.168.1.5"));
    }

    #[test]
    fn leading_dot_entry_matches_by_suffix() {
        assert!(no_proxy_covers(".internal.corp", "desktop.internal.corp"));
        assert!(!no_proxy_covers(".internal.corp", "internal.corp.evil.com"));
    }

    #[test]
    fn whitespace_and_empty_entries_are_ignored() {
        assert!(no_proxy_covers("  localhost , ", "127.0.0.1"));
        assert!(!no_proxy_covers(",,  ,", "127.0.0.1"));
    }

    // ── host extraction ──────────────────────────────────

    #[test]
    fn host_is_extracted_without_scheme_or_port() {
        assert_eq!(
            host_of("http://127.0.0.1:60606").as_deref(),
            Some("127.0.0.1")
        );
        assert_eq!(
            host_of("http://192.168.1.5:60606/mcp").as_deref(),
            Some("192.168.1.5")
        );
        assert_eq!(
            host_of("https://mcp.linkly.ai").as_deref(),
            Some("mcp.linkly.ai")
        );
    }

    #[test]
    fn ipv6_literals_keep_their_address() {
        // A bracketed literal must not be truncated at the colons inside it.
        assert_eq!(host_of("http://[::1]:60606").as_deref(), Some("::1"));
    }

    // ── T3.1: Mode 1 — Endpoint ──────────────────────────

    #[test]
    fn resolve_endpoint_mode() {
        let info = resolve(Some("http://192.168.1.100:60606/mcp"), None, false).unwrap();
        assert_eq!(info.mcp_url, "http://192.168.1.100:60606/mcp");
        assert_eq!(info.base_url, "http://192.168.1.100:60606");
        assert!(info.auth_header.is_none());
    }

    #[test]
    fn resolve_endpoint_with_token() {
        let info = resolve(Some("http://x:60606/mcp"), Some("tk"), false).unwrap();
        assert_eq!(info.mcp_url, "http://x:60606/mcp");
        assert_eq!(info.auth_header, Some("Bearer tk".to_string()));
    }

    #[test]
    fn resolve_endpoint_trailing_slash_cleaned() {
        let info = resolve(Some("http://x:60606/"), None, false).unwrap();
        assert_eq!(info.base_url, "http://x:60606");
        assert_eq!(info.mcp_url, "http://x:60606/mcp");
    }

    // ── T3.1: Mode 2 — Remote ────────────────────────────

    #[test]
    fn resolve_remote_no_credentials_fails() {
        with_temp_home("remote_no_cred", |_home| {
            let result = resolve(None, None, true);
            assert!(result.is_err());
        });
    }

    // ── T3.1: Mode 3 — Default (local port file) ────────

    #[test]
    fn resolve_default_with_port_file() {
        with_temp_home("default_port", |home| {
            let linkly_dir = home.join(".linkly");
            std::fs::create_dir_all(&linkly_dir).unwrap();
            std::fs::write(linkly_dir.join("port"), r#"{"port": 60606}"#).unwrap();

            let info = resolve(None, None, false).unwrap();
            assert_eq!(info.mcp_url, "http://127.0.0.1:60606/mcp");
            assert_eq!(info.base_url, "http://127.0.0.1:60606");
            assert!(info.auth_header.is_none());
        });
    }

    #[test]
    fn resolve_default_no_port_file_fails() {
        with_temp_home("default_no_port", |_home| {
            let result = resolve(None, None, false);
            assert!(result.is_err());
        });
    }

    // ── T3.3: Credentials round-trip ─────────────────────

    #[test]
    fn credentials_save_then_read() {
        with_temp_home("cred_roundtrip", |_home| {
            save_credentials_api_key("lkai_test_roundtrip_abcdef1234567890").unwrap();
            let read = read_credentials_api_key().unwrap();
            assert_eq!(
                read,
                Some("lkai_test_roundtrip_abcdef1234567890".to_string())
            );
        });
    }

    #[test]
    fn credentials_auto_creates_directory() {
        with_temp_home("cred_mkdir", |home| {
            assert!(!home.join(".linkly").exists());
            save_credentials_api_key("lkai_test").unwrap();
            assert!(home.join(".linkly").exists());
        });
    }

    #[test]
    fn credentials_reads_legacy_format() {
        with_temp_home("cred_legacy", |home| {
            let dir = home.join(".linkly");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("credentials.json"),
                r#"{"apiKey":"lkai_legacy_format_test1234567890ab"}"#,
            )
            .unwrap();
            assert_eq!(
                read_credentials_api_key().unwrap(),
                Some("lkai_legacy_format_test1234567890ab".to_string())
            );
        });
    }

    #[test]
    fn credentials_reads_new_format() {
        with_temp_home("cred_new_fmt", |home| {
            let dir = home.join(".linkly");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("credentials.json"),
                r#"{"remotes":[{"name":"default","key":"lkai_new_format_test01234567890ab"}]}"#,
            )
            .unwrap();
            assert_eq!(
                read_credentials_api_key().unwrap(),
                Some("lkai_new_format_test01234567890ab".to_string())
            );
        });
    }

    #[test]
    fn credentials_corrupted_returns_error() {
        with_temp_home("cred_corrupt", |home| {
            let dir = home.join(".linkly");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("credentials.json"), "not json").unwrap();
            let result = read_credentials_api_key();
            assert!(result.is_err(), "corrupted JSON should return Err");
        });
    }

    #[test]
    fn credentials_missing_returns_none() {
        with_temp_home("cred_missing", |_home| {
            let result = read_credentials_api_key().unwrap();
            assert_eq!(result, None, "missing file should return Ok(None)");
        });
    }

    // ── T3.4: Clap 参数冲突 ──────────────────────────────

    #[test]
    fn clap_rejects_endpoint_and_remote_together() {
        use clap::Parser;
        let result = crate::cli::Cli::try_parse_from([
            "linkly",
            "search",
            "test",
            "--endpoint",
            "http://x",
            "--token",
            "tk",
            "--remote",
        ]);
        assert!(result.is_err(), "--endpoint and --remote should conflict");
    }

    #[test]
    fn clap_rejects_token_without_endpoint() {
        use clap::Parser;
        let result =
            crate::cli::Cli::try_parse_from(["linkly", "search", "test", "--token", "abc"]);
        assert!(result.is_err(), "--token requires --endpoint");
    }

    #[test]
    fn clap_rejects_endpoint_without_token() {
        use clap::Parser;
        let result =
            crate::cli::Cli::try_parse_from(["linkly", "search", "test", "--endpoint", "http://x"]);
        assert!(result.is_err(), "--endpoint requires --token");
    }

    #[test]
    fn clap_accepts_endpoint_with_token() {
        use clap::Parser;
        let result = crate::cli::Cli::try_parse_from([
            "linkly",
            "search",
            "test",
            "--endpoint",
            "http://x",
            "--token",
            "tk",
        ]);
        assert!(result.is_ok(), "--endpoint + --token should be accepted");
    }
}
