use anyhow::{Context, Result, bail};

/// Resolved connection info for the Linkly AI desktop app.
pub struct ConnectionInfo {
    /// MCP endpoint URL (e.g. http://127.0.0.1:60606/mcp)
    pub mcp_url: String,
    /// Base URL (e.g. http://127.0.0.1:60606)
    pub base_url: String,
}

/// Resolve the MCP endpoint.
///
/// Priority: `--endpoint` flag > `~/.linkly/port` file > error.
pub fn resolve(endpoint: Option<&str>) -> Result<ConnectionInfo> {
    if let Some(ep) = endpoint {
        let base = ep
            .trim_end_matches('/')
            .trim_end_matches("/mcp")
            .to_string();
        let mcp = format!("{}/mcp", base);
        return Ok(ConnectionInfo {
            mcp_url: mcp,
            base_url: base,
        });
    }

    // Try reading ~/.linkly/port
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
    })
}
