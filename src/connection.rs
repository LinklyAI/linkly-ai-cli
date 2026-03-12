use anyhow::{Context, Result, bail};

/// Remote tunnel endpoint
const REMOTE_MCP_URL: &str = "https://mcp.linkly.ai/mcp";
const REMOTE_BASE_URL: &str = "https://mcp.linkly.ai";

/// Credentials file path: ~/.linkly/credentials.json
/// Format: { "apiKey": "lkai_xxx" }
const CREDENTIALS_FILE: &str = "credentials.json";

/// Resolved connection info for the Linkly AI desktop app.
pub struct ConnectionInfo {
    /// MCP endpoint URL (e.g. http://127.0.0.1:60606/mcp)
    pub mcp_url: String,
    /// Base URL (e.g. http://127.0.0.1:60606)
    pub base_url: String,
    /// Optional auth header value (e.g. "Bearer lkai_xxx")
    pub auth_header: Option<String>,
}

/// Resolve the MCP endpoint.
///
/// Three modes:
/// 1. `--endpoint` + optional `--token` → explicit endpoint (LAN mode)
/// 2. `--remote` → read ~/.linkly/credentials.json, connect to mcp.linkly.ai
/// 3. Default → read ~/.linkly/port (local mode, no auth)
pub fn resolve(
    endpoint: Option<&str>,
    token: Option<&str>,
    remote: bool,
) -> Result<ConnectionInfo> {
    // Mode 1: explicit endpoint (+ optional token for LAN auth)
    if let Some(ep) = endpoint {
        let base = ep
            .trim_end_matches('/')
            .trim_end_matches("/mcp")
            .to_string();
        let mcp = format!("{}/mcp", base);
        let auth_header = token.map(|t| format!("Bearer {}", t));
        return Ok(ConnectionInfo {
            mcp_url: mcp,
            base_url: base,
            auth_header,
        });
    }

    // Mode 2: remote tunnel via mcp.linkly.ai
    if remote {
        let api_key = token
            .map(|t| t.to_string())
            .or_else(read_credentials_api_key)
            .context(
                "No API key found for remote mode.\n\
                 Run `linkly auth set-key <your-api-key>` first, or use --token <key>.",
            )?;

        return Ok(ConnectionInfo {
            mcp_url: REMOTE_MCP_URL.to_string(),
            base_url: REMOTE_BASE_URL.to_string(),
            auth_header: Some(format!("Bearer {}", api_key)),
        });
    }

    // Mode 3: local — read ~/.linkly/port
    let port_file = dirs::home_dir()
        .map(|h| h.join(".linkly").join("port"))
        .context("Cannot determine home directory")?;

    let content = std::fs::read_to_string(&port_file).with_context(|| {
        format!(
            "Linkly AI app does not appear to be running.\n\
             Port file not found: {}\n\
             Start the Linkly AI desktop app first, or use --endpoint to connect manually.",
            port_file.display()
        )
    })?;

    let parsed: serde_json::Value =
        serde_json::from_str(&content).context("Invalid port file format")?;

    let port = parsed["port"]
        .as_u64()
        .context("Port file missing 'port' field")?;

    if port == 0 || port > 65535 {
        bail!("Invalid port number in port file: {}", port);
    }

    let base = format!("http://127.0.0.1:{}", port);
    let mcp = format!("{}/mcp", base);

    Ok(ConnectionInfo {
        mcp_url: mcp,
        base_url: base,
        auth_header: None,
    })
}

/// Read API key from ~/.linkly/credentials.json
pub(crate) fn read_credentials_api_key() -> Option<String> {
    let path = dirs::home_dir()?.join(".linkly").join(CREDENTIALS_FILE);
    let content = std::fs::read_to_string(path).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&content).ok()?;
    parsed["apiKey"].as_str().map(|s| s.to_string())
}

/// Save API key to ~/.linkly/credentials.json
pub fn save_credentials_api_key(api_key: &str) -> Result<()> {
    let dir = dirs::home_dir()
        .context("Cannot determine home directory")?
        .join(".linkly");

    std::fs::create_dir_all(&dir)
        .with_context(|| format!("Failed to create directory: {}", dir.display()))?;

    let path = dir.join(CREDENTIALS_FILE);
    let content = serde_json::json!({ "apiKey": api_key });
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
    fn resolve_remote_with_token() {
        let info = resolve(None, Some("lkai_xxx"), true).unwrap();
        assert_eq!(info.mcp_url, "https://mcp.linkly.ai/mcp");
        assert_eq!(info.base_url, "https://mcp.linkly.ai");
        assert_eq!(info.auth_header, Some("Bearer lkai_xxx".to_string()));
    }

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
            let read = read_credentials_api_key();
            assert_eq!(read, Some("lkai_test_roundtrip_abcdef1234567890".to_string()));
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
    fn credentials_corrupted_returns_none() {
        with_temp_home("cred_corrupt", |home| {
            let dir = home.join(".linkly");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("credentials.json"), "not json").unwrap();
            assert_eq!(read_credentials_api_key(), None);
        });
    }

    // ── T3.4: Clap 参数冲突 ──────────────────────────────

    #[test]
    fn clap_rejects_endpoint_and_remote_together() {
        use clap::Parser;
        let result = crate::cli::Cli::try_parse_from([
            "linkly", "--endpoint", "http://x", "--remote", "search", "test",
        ]);
        assert!(result.is_err(), "--endpoint and --remote should conflict");
    }
}
