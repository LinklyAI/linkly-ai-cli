use anyhow::{bail, Context, Result};
use serde::Deserialize;

/// Remote tunnel endpoint
const REMOTE_MCP_URL: &str = "https://mcp.linkly.ai/mcp";
pub(crate) const REMOTE_BASE_URL: &str = "https://mcp.linkly.ai";

/// Credentials file path: <config dir>/credentials.json (see `config_dir`)
/// New format: { "remotes": [{ "name": "default", "key": "lkai_xxx" }],
///               "lan": { "endpoint": "http://192.168.1.100:60606", "token": "..." } }
/// Legacy format (still readable): { "apiKey": "lkai_xxx" }
///
/// Each section is owned by one command (`auth set-key` / `auth set-key --lan`),
/// and every write merges into the existing file, so saving one never drops the
/// other — or any field a newer CLI version added that this one doesn't know.
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
    /// `saved` is true when the endpoint and token came from the credentials
    /// file (`--lan`) rather than from `--endpoint` / `--token`, so hints can
    /// point the user back at the same spelling they used.
    Lan {
        endpoint: String,
        saved: bool,
    },
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
    /// Sanitized `--client` value, sent as `X-Linkly-Client` on every request.
    /// `None` when the caller did not name itself, or named itself something
    /// we cannot put in a header.
    pub client_name: Option<String>,
}

/// Header carrying the calling application's name to Desktop's access log.
///
/// The same name on all three routes: local and LAN reach Desktop's middleware
/// directly, and on `--remote` the cloud gateway passes the header through
/// (API-key auth has no client identity of its own to look up, so the caller's
/// own claim is all there is — and since the key belongs to the user, a false
/// name could only mislead the user about their own calls).
pub const CLIENT_HEADER: &str = "x-linkly-client";

/// Upper bound on the client name, matching Desktop's `sanitize_app_name`.
const MAX_CLIENT_NAME_CHARS: usize = 64;

/// Names that mean something specific inside Desktop: they are how Desktop
/// labels its own built-in chatbot and its internal call paths. A third party
/// claiming one of them would show up in the access log as Linkly itself.
const RESERVED_CLIENT_NAMES: [&str; 3] = ["linkly-chatbot", "linkly", "external-mcp"];

/// Clean up a `--client` value, or give up on it.
///
/// Giving up is deliberately silent: the name is a label for the user's access
/// log, and refusing to run a search because the label had an emoji in it would
/// trade a working command for a cosmetic detail. The call still goes through,
/// it just shows up under its connection source instead.
///
/// Non-ASCII is dropped whole rather than filtered down to its ASCII part —
/// `思源笔记 Pro` reduced to `Pro` would tell the user that an application
/// called Pro read their files, which is worse than telling them nothing.
pub fn sanitize_client_name(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.chars().count() > MAX_CLIENT_NAME_CHARS {
        return None;
    }
    if !trimmed.chars().all(|c| c.is_ascii_graphic() || c == ' ') {
        return None;
    }
    let lowered = trimmed.to_ascii_lowercase();
    if RESERVED_CLIENT_NAMES.contains(&lowered.as_str()) {
        return None;
    }
    Some(trimmed.to_string())
}

impl ConnectionInfo {
    /// Generate a `linkly doctor` command hint matching the current connection mode.
    pub fn doctor_hint(&self) -> String {
        match &self.mode {
            ConnectionMode::Local => {
                "Run 'linkly doctor' to diagnose local connection issues.".to_string()
            }
            ConnectionMode::Lan { saved: true, .. } => {
                "Run 'linkly doctor --lan' to diagnose LAN connection issues.".to_string()
            }
            ConnectionMode::Lan { endpoint, .. } => {
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
/// Four modes:
/// 1. `--endpoint` + `--token` → explicit endpoint (LAN mode, both required by clap)
/// 2. `--lan` → LAN endpoint and token saved by `auth set-key --lan`
/// 3. `--remote` → read ~/.linkly/credentials.json, connect to mcp.linkly.ai
///    (`token` is always ignored in this mode; authentication uses credentials file)
/// 4. Default → read ~/.linkly/port (local mode, no auth)
///
/// Note: `endpoint`/`token`, `lan` and `remote` are mutually exclusive
/// (enforced by clap). Without any of them the CLI only ever talks to the local
/// Desktop — a saved LAN connection is never picked up implicitly.
///
/// `client` is the caller's `--client` value; it rides along on every mode and
/// never affects which one is chosen.
pub fn resolve(
    endpoint: Option<&str>,
    token: Option<&str>,
    lan: bool,
    remote: bool,
    client: Option<&str>,
) -> Result<ConnectionInfo> {
    let client_name = client.and_then(sanitize_client_name);
    // Mode 1: explicit endpoint + token for LAN auth
    if let Some(ep) = endpoint {
        return Ok(lan_connection(ep, token, false, client_name));
    }

    // Mode 2: LAN endpoint + token saved in the credentials file
    if lan {
        let saved = read_lan_credentials()?.context(
            "No LAN connection saved.\n\n\
                 To set up:\n  \
                   1. Find the LAN address and token in Linkly AI Desktop > Settings > MCP\n  \
                   2. Run: linkly auth set-key <token> --lan --endpoint <url>\n\n\
                 Run 'linkly doctor --lan' to diagnose LAN connection issues.",
        )?;
        return Ok(lan_connection(
            &saved.endpoint,
            Some(&saved.token),
            true,
            client_name,
        ));
    }

    // Mode 3: remote tunnel via mcp.linkly.ai
    if remote {
        let api_key = read_credentials_api_key()?.context(concat!(
            "No API key configured for remote mode.\n\n",
            "To set up, run: linkly auth login\n  ",
            crate::agent_login_hint!(),
            "\n  (or paste a key from https://linkly.ai Dashboard > API Keys: linkly auth set-key <your-api-key>)\n\n",
            "Run 'linkly doctor --remote' to diagnose remote connection issues."
        ))?;

        return Ok(ConnectionInfo {
            mcp_url: REMOTE_MCP_URL.to_string(),
            base_url: REMOTE_BASE_URL.to_string(),
            auth_header: Some(format!("Bearer {}", api_key)),
            is_remote: true,
            mode: ConnectionMode::Remote,
            client_name,
        });
    }

    // Mode 4: local — read ~/.linkly/port
    let port_file = dirs::home_dir()
        .map(|h| h.join(".linkly").join("port"))
        .context("Cannot determine home directory")?;

    let content = std::fs::read_to_string(&port_file).map_err(|_| {
        anyhow::anyhow!(
            "Linkly AI Desktop is not running on this machine.\n\n\
             How to connect:\n  \
               Local:   Launch Linkly AI Desktop on this machine\n  \
               Remote:  linkly <command> --remote  (sign in first: linkly auth login)\n  \
               LAN:     linkly <command> --lan  (save it first: linkly auth set-key <token> --lan --endpoint <url>;\n\
             \x20          find the address and token in Desktop > Settings > MCP)\n\n\
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
        client_name,
    })
}

/// Build a LAN connection from an endpoint URL and an optional bearer token.
fn lan_connection(
    endpoint: &str,
    token: Option<&str>,
    saved: bool,
    client_name: Option<String>,
) -> ConnectionInfo {
    let base = normalize_endpoint(endpoint);
    ConnectionInfo {
        mcp_url: format!("{}/mcp", base),
        auth_header: token.map(|t| format!("Bearer {}", t)),
        is_remote: false,
        mode: ConnectionMode::Lan {
            endpoint: base.clone(),
            saved,
        },
        base_url: base,
        client_name,
    }
}

/// Reduce an endpoint URL to its base: strip trailing `/` and a `/mcp` suffix,
/// so `http://x:60606`, `http://x:60606/` and `http://x:60606/mcp` all agree.
pub fn normalize_endpoint(endpoint: &str) -> String {
    let trimmed = endpoint.trim_end_matches('/');
    trimmed.strip_suffix("/mcp").unwrap_or(trimmed).to_string()
}

/// LAN connection saved by `linkly auth set-key <token> --lan --endpoint <url>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanCredentials {
    /// Base URL, already normalized (no trailing `/` or `/mcp`)
    pub endpoint: String,
    /// Desktop's LAN access token
    pub token: String,
}

/// Read API key from ~/.linkly/credentials.json
/// Supports new format (remotes[0].key) with fallback to legacy format (apiKey).
///
/// Returns:
/// - `Ok(Some(key))` — key found and read successfully
/// - `Ok(None)` — file or key does not exist (user never ran `auth set-key`)
/// - `Err(...)` — file exists but is corrupted or unreadable
pub(crate) fn read_credentials_api_key() -> Result<Option<String>> {
    let Some(parsed) = read_credentials_file()? else {
        return Ok(None);
    };

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

/// Where the stored remote key came from (`Some("login")` for `auth login`),
/// `None` when unmarked — `set-key`, a legacy file, or no key at all.
pub(crate) fn read_remote_key_source() -> Result<Option<String>> {
    let Some(parsed) = read_credentials_file()? else {
        return Ok(None);
    };
    Ok(parsed["remotes"]
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|entry| entry["source"].as_str())
        .map(str::to_string))
}

/// Read the LAN connection from ~/.linkly/credentials.json.
///
/// Same contract as [`read_credentials_api_key`]: `Ok(None)` when nothing is
/// saved, `Err` only when the file itself is unreadable or corrupted. A `lan`
/// section missing either field counts as not saved.
pub(crate) fn read_lan_credentials() -> Result<Option<LanCredentials>> {
    let Some(parsed) = read_credentials_file()? else {
        return Ok(None);
    };
    let lan = &parsed["lan"];
    match (lan["endpoint"].as_str(), lan["token"].as_str()) {
        (Some(endpoint), Some(token)) if !endpoint.is_empty() && !token.is_empty() => {
            Ok(Some(LanCredentials {
                endpoint: endpoint.to_string(),
                token: token.to_string(),
            }))
        }
        _ => Ok(None),
    }
}

/// Read and parse the credentials file. `Ok(None)` when it does not exist.
fn read_credentials_file() -> Result<Option<serde_json::Value>> {
    let path = credentials_path()?;
    let Some(content) = read_credentials_text(&path)? else {
        return Ok(None);
    };
    let parsed = serde_json::from_str(&content).with_context(|| {
        format!(
            "Credentials file is corrupted: {}\n\
             Run 'linkly auth set-key <your-api-key>' to fix it.\n\
             Get your API key from https://linkly.ai (Dashboard > API Keys).",
            path.display()
        )
    })?;
    Ok(Some(parsed))
}

/// Raw file contents. `Ok(None)` when the file does not exist; any other I/O
/// failure is an error, so callers never mistake "unreadable" for "empty".
fn read_credentials_text(path: &std::path::Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(c) => Ok(Some(c)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(anyhow::anyhow!(
            "Failed to read credentials file {}: {}",
            path.display(),
            e
        )),
    }
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

/// Environment variable that relocates the CLI's own state (credentials and
/// the pending browser login). One directory holds one account, so an agent
/// serving several users gives each of them their own directory.
///
/// It deliberately does **not** move `~/.linkly/port`: that file is written by
/// the Desktop app, and local discovery must keep finding it.
pub const CONFIG_DIR_ENV: &str = "LINKLY_CONFIG_DIR";

/// Directory holding credentials.json: `$LINKLY_CONFIG_DIR`, or `~/.linkly`.
pub fn config_dir() -> Result<std::path::PathBuf> {
    match std::env::var_os(CONFIG_DIR_ENV) {
        Some(dir) if !dir.is_empty() => Ok(std::path::PathBuf::from(dir)),
        _ => Ok(dirs::home_dir()
            .context("Cannot determine home directory")?
            .join(".linkly")),
    }
}

/// Absolute path of the credentials file (`<config dir>/credentials.json`).
///
/// Single source of truth so read / write / delete can never disagree about
/// where the file lives — and so `auth status` can show the user the real path
/// instead of a hardcoded string.
pub fn credentials_path() -> Result<std::path::PathBuf> {
    Ok(config_dir()?.join(CREDENTIALS_FILE))
}

/// Fields holding the remote API key: the current `remotes` array and the
/// legacy `apiKey` that older CLIs wrote.
const REMOTE_FIELDS: [&str; 2] = ["remotes", "apiKey"];
/// Field holding the saved LAN connection.
const LAN_FIELDS: [&str; 1] = ["lan"];

/// Remove the remote API key, keeping everything else in the file.
///
/// Returns whether a key was actually removed. A missing file or key is
/// **not** an error: the caller asked for "no stored key", and that already holds.
pub fn delete_remote_credentials() -> Result<bool> {
    remove_credential_fields(&REMOTE_FIELDS)
}

/// Remove the saved LAN connection, keeping everything else in the file.
pub fn delete_lan_credentials() -> Result<bool> {
    remove_credential_fields(&LAN_FIELDS)
}

/// `source` marker on a remote key that `linkly auth login` obtained. Only
/// such keys are revoked remotely on logout: one pasted in with `set-key` may
/// also serve other clients.
pub const LOGIN_KEY_SOURCE: &str = "login";

/// Save an API key pasted in with `auth set-key` (new format with remotes
/// array), leaving the LAN section and any unknown fields untouched.
pub fn save_credentials_api_key(api_key: &str) -> Result<()> {
    save_remote_key(api_key, None)
}

/// Save an API key issued by `auth login`, marked so logout can revoke it.
pub fn save_login_api_key(api_key: &str) -> Result<()> {
    save_remote_key(api_key, Some(LOGIN_KEY_SOURCE))
}

fn save_remote_key(api_key: &str, source: Option<&str>) -> Result<()> {
    let mut map = load_credentials_for_update()?;
    // Drop the legacy field so the file holds a single key, not two that can
    // disagree.
    map.remove("apiKey");
    let mut entry = serde_json::json!({ "name": "default", "key": api_key });
    if let Some(source) = source {
        entry["source"] = serde_json::json!(source);
    }
    map.insert("remotes".to_string(), serde_json::json!([entry]));
    write_credentials_file(map)
}

/// Save the LAN connection, leaving the remote key and any unknown fields
/// untouched. `endpoint` is stored normalized.
pub fn save_lan_credentials(endpoint: &str, token: &str) -> Result<()> {
    let mut map = load_credentials_for_update()?;
    map.insert(
        "lan".to_string(),
        serde_json::json!({ "endpoint": normalize_endpoint(endpoint), "token": token }),
    );
    write_credentials_file(map)
}

/// Current file contents as a JSON object, ready to be edited and written back.
///
/// A missing file starts empty. So does a corrupted one: nothing in it can be
/// kept, and `set-key` is the documented fix for that state. An unreadable file
/// is an error — overwriting it could destroy credentials we simply failed to read.
fn load_credentials_for_update() -> Result<serde_json::Map<String, serde_json::Value>> {
    let path = credentials_path()?;
    let Some(content) = read_credentials_text(&path)? else {
        return Ok(serde_json::Map::new());
    };
    match serde_json::from_str(&content) {
        Ok(serde_json::Value::Object(map)) => Ok(map),
        _ => Ok(serde_json::Map::new()),
    }
}

/// Remove `fields` from the credentials file; delete the file once nothing is
/// left in it. Returns whether any field was present.
///
/// A corrupted file is deleted outright (reported as removed): nothing in it
/// was usable anyway, and this is what `auth logout` always did with it.
fn remove_credential_fields(fields: &[&str]) -> Result<bool> {
    let path = credentials_path()?;
    let Some(content) = read_credentials_text(&path)? else {
        return Ok(false);
    };
    let mut map = match serde_json::from_str(&content) {
        Ok(serde_json::Value::Object(map)) => map,
        _ => {
            remove_credentials_file(&path)?;
            return Ok(true);
        }
    };

    // Not `any()`: it would stop at the first hit and leave the rest behind.
    let mut removed = false;
    for field in fields {
        removed |= map.remove(*field).is_some();
    }
    if !removed {
        return Ok(false);
    }
    if map.is_empty() {
        remove_credentials_file(&path)?;
    } else {
        write_credentials_file(map)?;
    }
    Ok(true)
}

fn remove_credentials_file(path: &std::path::Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(anyhow::anyhow!(
            "Failed to remove credentials file {}: {}",
            path.display(),
            e
        )),
    }
}

/// Write the credentials file atomically with owner-only permissions.
///
/// The JSON goes to a temp file in the same directory, which is then renamed
/// over the target: a crash or full disk mid-write leaves the previous file
/// intact instead of a truncated one. `tempfile` creates the file 0600 on Unix,
/// and `persist` replaces an existing target on Windows too.
fn write_credentials_file(map: serde_json::Map<String, serde_json::Value>) -> Result<()> {
    use std::io::Write;

    let path = credentials_path()?;
    let dir = path
        .parent()
        .context("Credentials path has no parent directory")?;

    std::fs::create_dir_all(dir)
        .with_context(|| format!("Failed to create directory: {}", dir.display()))?;

    let json_bytes = serde_json::to_string_pretty(&serde_json::Value::Object(map))?;

    let mut tmp = tempfile::Builder::new()
        .prefix(".credentials-")
        .suffix(".tmp")
        .tempfile_in(dir)
        .with_context(|| format!("Failed to create a temp file in {}", dir.display()))?;
    tmp.write_all(json_bytes.as_bytes())
        .and_then(|()| tmp.as_file().sync_all())
        .with_context(|| format!("Failed to write credentials: {}", path.display()))?;
    tmp.persist(&path)
        .map_err(|e| e.error)
        .with_context(|| format!("Failed to write credentials: {}", path.display()))?;

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
        let info = resolve(
            Some("http://192.168.1.100:60606/mcp"),
            None,
            false,
            false,
            None,
        )
        .unwrap();
        assert_eq!(info.mcp_url, "http://192.168.1.100:60606/mcp");
        assert_eq!(info.base_url, "http://192.168.1.100:60606");
        assert!(info.auth_header.is_none());
    }

    #[test]
    fn resolve_endpoint_with_token() {
        let info = resolve(Some("http://x:60606/mcp"), Some("tk"), false, false, None).unwrap();
        assert_eq!(info.mcp_url, "http://x:60606/mcp");
        assert_eq!(info.auth_header, Some("Bearer tk".to_string()));
    }

    #[test]
    fn resolve_endpoint_trailing_slash_cleaned() {
        let info = resolve(Some("http://x:60606/"), None, false, false, None).unwrap();
        assert_eq!(info.base_url, "http://x:60606");
        assert_eq!(info.mcp_url, "http://x:60606/mcp");
    }

    // ── T3.1: Mode 2 — Remote ────────────────────────────

    #[test]
    fn resolve_remote_no_credentials_fails() {
        with_temp_home("remote_no_cred", |_home| {
            let result = resolve(None, None, false, true, None);
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

            let info = resolve(None, None, false, false, None).unwrap();
            assert_eq!(info.mcp_url, "http://127.0.0.1:60606/mcp");
            assert_eq!(info.base_url, "http://127.0.0.1:60606");
            assert!(info.auth_header.is_none());
        });
    }

    #[test]
    fn resolve_default_no_port_file_fails() {
        with_temp_home("default_no_port", |_home| {
            let result = resolve(None, None, false, false, None);
            assert!(result.is_err());
        });
    }

    // ── Mode 2 — saved LAN connection ───────────────────

    #[test]
    fn resolve_lan_uses_saved_endpoint_and_token() {
        with_temp_home("lan_saved", |_home| {
            save_lan_credentials("http://192.168.1.9:60606/mcp", "tk").unwrap();
            let info = resolve(None, None, true, false, None).unwrap();
            assert_eq!(info.mcp_url, "http://192.168.1.9:60606/mcp");
            assert_eq!(info.base_url, "http://192.168.1.9:60606");
            assert_eq!(info.auth_header.as_deref(), Some("Bearer tk"));
            assert!(!info.is_remote);
            assert!(matches!(info.mode, ConnectionMode::Lan { saved: true, .. }));
        });
    }

    #[test]
    fn resolve_lan_without_saved_connection_explains_set_key() {
        with_temp_home("lan_unsaved", |_home| {
            let err = resolve(None, None, true, false, None)
                .err()
                .expect("nothing saved");
            assert!(err.to_string().contains("set-key <token> --lan --endpoint"));
        });
    }

    #[test]
    fn resolve_default_never_falls_back_to_saved_lan() {
        with_temp_home("lan_not_implicit", |_home| {
            // No local Desktop, but a LAN connection is saved: the default
            // mode must still fail rather than silently go to the LAN.
            save_lan_credentials("http://192.168.1.9:60606", "tk").unwrap();
            assert!(resolve(None, None, false, false, None).is_err());
        });
    }

    #[test]
    fn explicit_endpoint_wins_over_saved_lan() {
        with_temp_home("lan_explicit_wins", |_home| {
            save_lan_credentials("http://192.168.1.9:60606", "saved").unwrap();
            let info = resolve(
                Some("http://10.0.0.2:60606"),
                Some("given"),
                false,
                false,
                None,
            )
            .unwrap();
            assert_eq!(info.base_url, "http://10.0.0.2:60606");
            assert_eq!(info.auth_header.as_deref(), Some("Bearer given"));
            assert!(matches!(
                info.mode,
                ConnectionMode::Lan { saved: false, .. }
            ));
        });
    }

    // ── LINKLY_CONFIG_DIR ────────────────────────────────

    #[test]
    fn config_dir_env_relocates_credentials_but_not_the_port_file() {
        with_temp_home("config_dir_env", |home| {
            let custom = home.join("tenant-a");
            std::env::set_var(CONFIG_DIR_ENV, &custom);
            save_credentials_api_key("lkai_tenant_a").unwrap();
            let in_custom = custom.join("credentials.json").exists();
            let in_default = home.join(".linkly").join("credentials.json").exists();

            // Local discovery still reads ~/.linkly/port.
            std::fs::create_dir_all(home.join(".linkly")).unwrap();
            std::fs::write(home.join(".linkly").join("port"), r#"{"port": 60606}"#).unwrap();
            let local = resolve(None, None, false, false, None);

            std::env::remove_var(CONFIG_DIR_ENV);
            assert!(in_custom, "credentials must land in $LINKLY_CONFIG_DIR");
            assert!(!in_default, "the default directory must stay untouched");
            assert_eq!(local.unwrap().base_url, "http://127.0.0.1:60606");
        });
    }

    #[test]
    fn empty_config_dir_env_falls_back_to_home() {
        with_temp_home("config_dir_empty", |home| {
            std::env::set_var(CONFIG_DIR_ENV, "");
            let dir = config_dir().unwrap();
            std::env::remove_var(CONFIG_DIR_ENV);
            assert_eq!(dir, home.join(".linkly"));
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
    fn saving_keeps_fields_this_version_does_not_know() {
        with_temp_home("cred_unknown_fields", |home| {
            let dir = home.join(".linkly");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("credentials.json"),
                r#"{"remotes":[{"name":"default","key":"lkai_old"}],"future":{"x":1}}"#,
            )
            .unwrap();

            save_lan_credentials("http://192.168.1.9:60606", "tk").unwrap();
            save_credentials_api_key("lkai_new").unwrap();

            let raw: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(dir.join("credentials.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(raw["future"]["x"], 1);
            assert_eq!(raw["remotes"][0]["key"], "lkai_new");
            assert_eq!(raw["lan"]["token"], "tk");
        });
    }

    #[test]
    fn login_keys_are_marked_and_set_key_clears_the_mark() {
        with_temp_home("cred_source", |_home| {
            save_login_api_key("lkai_from_login").unwrap();
            assert_eq!(
                read_remote_key_source().unwrap().as_deref(),
                Some(LOGIN_KEY_SOURCE)
            );
            assert_eq!(
                read_credentials_api_key().unwrap().as_deref(),
                Some("lkai_from_login")
            );

            save_credentials_api_key("lkai_pasted").unwrap();
            assert_eq!(read_remote_key_source().unwrap(), None);
        });
    }

    #[test]
    fn saving_lan_keeps_a_legacy_api_key_readable() {
        with_temp_home("cred_legacy_plus_lan", |home| {
            let dir = home.join(".linkly");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("credentials.json"), r#"{"apiKey":"lkai_legacy"}"#).unwrap();

            save_lan_credentials("http://192.168.1.9:60606", "tk").unwrap();
            assert_eq!(
                read_credentials_api_key().unwrap().as_deref(),
                Some("lkai_legacy")
            );
            assert!(read_lan_credentials().unwrap().is_some());
        });
    }

    #[test]
    fn saving_over_a_corrupted_file_starts_fresh() {
        with_temp_home("cred_corrupt_overwrite", |home| {
            let dir = home.join(".linkly");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("credentials.json"), "not json").unwrap();

            save_credentials_api_key("lkai_fixed").unwrap();
            assert_eq!(
                read_credentials_api_key().unwrap().as_deref(),
                Some("lkai_fixed")
            );
        });
    }

    #[test]
    fn deleting_from_a_corrupted_file_removes_it() {
        with_temp_home("cred_corrupt_delete", |home| {
            let dir = home.join(".linkly");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("credentials.json"), "not json").unwrap();

            assert!(delete_remote_credentials().unwrap());
            assert!(!dir.join("credentials.json").exists());
        });
    }

    #[cfg(unix)]
    #[test]
    fn credentials_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        with_temp_home("cred_mode", |home| {
            let path = home.join(".linkly").join("credentials.json");
            save_credentials_api_key("lkai_x").unwrap();
            // Replacing an existing file must not loosen the mode either.
            save_lan_credentials("http://192.168.1.9:60606", "tk").unwrap();
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
            // No temp files left behind next to it.
            let entries: Vec<_> = std::fs::read_dir(path.parent().unwrap()).unwrap().collect();
            assert_eq!(entries.len(), 1);
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

    #[test]
    fn clap_accepts_lan_alone() {
        use clap::Parser;
        let cli = crate::cli::Cli::try_parse_from(["linkly", "search", "test", "--lan"]);
        assert!(cli.is_ok(), "--lan should be accepted on its own");
    }

    #[test]
    fn clap_rejects_lan_with_other_targets() {
        use clap::Parser;
        for extra in [
            &["--remote"][..],
            &["--endpoint", "http://x", "--token", "tk"][..],
        ] {
            let mut args = vec!["linkly", "search", "test", "--lan"];
            args.extend_from_slice(extra);
            assert!(
                crate::cli::Cli::try_parse_from(args).is_err(),
                "--lan must conflict with {extra:?}"
            );
        }
    }

    #[test]
    fn client_name_keeps_the_names_agents_actually_pass() {
        for name in [
            "claude-code",
            "Cursor",
            "Cherry Studio",
            "codex-cli",
            "n8n",
            "ChatGPT",
        ] {
            assert_eq!(
                sanitize_client_name(name).as_deref(),
                Some(name),
                "{name} should survive"
            );
        }
    }

    #[test]
    fn client_name_trims_surrounding_space() {
        assert_eq!(
            sanitize_client_name("  Cursor  ").as_deref(),
            Some("Cursor")
        );
    }

    #[test]
    fn client_name_drops_non_ascii_whole() {
        // Not filtered down to its ASCII part: "Pro" would name an application
        // that does not exist, which reads as fact in the user's access log.
        assert_eq!(sanitize_client_name("思源笔记 Pro"), None);
        assert_eq!(sanitize_client_name("Café"), None);
        assert_eq!(sanitize_client_name("Claude \u{1F916}"), None);
    }

    #[test]
    fn client_name_drops_control_characters() {
        assert_eq!(sanitize_client_name("Cur\nsor"), None);
        assert_eq!(sanitize_client_name("Cur\tsor"), None);
    }

    #[test]
    fn client_name_drops_reserved_names() {
        for name in ["linkly", "Linkly", "linkly-chatbot", "external-mcp"] {
            assert_eq!(sanitize_client_name(name), None, "{name} is reserved");
        }
    }

    #[test]
    fn client_name_drops_empty_and_overlong() {
        assert_eq!(sanitize_client_name(""), None);
        assert_eq!(sanitize_client_name("   "), None);
        assert_eq!(sanitize_client_name(&"a".repeat(65)), None);
        assert_eq!(
            sanitize_client_name(&"a".repeat(64)).as_deref(),
            Some("a".repeat(64).as_str())
        );
    }

    #[test]
    fn resolve_carries_a_usable_client_name_and_drops_an_unusable_one() {
        let conn = resolve(
            Some("http://192.168.1.9:60606/mcp"),
            Some("tk"),
            false,
            false,
            Some("claude-code"),
        )
        .expect("LAN mode resolves from args alone");
        assert_eq!(conn.client_name.as_deref(), Some("claude-code"));

        let conn = resolve(
            Some("http://192.168.1.9:60606/mcp"),
            Some("tk"),
            false,
            false,
            Some("思源笔记"),
        )
        .expect("an unusable name must not fail the connection");
        assert_eq!(conn.client_name, None);
    }

    #[test]
    fn clap_takes_client_after_the_subcommand() {
        use clap::Parser;
        let cli = crate::cli::Cli::try_parse_from([
            "linkly",
            "search",
            "test",
            "--client",
            "claude-code",
        ])
        .expect("--client is global");
        assert_eq!(cli.client.as_deref(), Some("claude-code"));
    }
}
