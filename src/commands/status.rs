use anyhow::Result;
use owo_colors::OwoColorize;
use serde::Deserialize;

use crate::connection::{ConnectionInfo, RemoteHealthResponse};
use crate::output;
use crate::version_check;

/// Local desktop health response schema (GET /health)
#[derive(Deserialize)]
struct HealthResponse {
    version: String,
    doc_count: u64,
    mcp_endpoint: Option<String>,
    index_status: String,
}

pub async fn run(conn: &ConnectionInfo, json_mode: bool) -> Result<()> {
    if conn.is_remote {
        return run_remote(conn, json_mode).await;
    }
    run_local(conn, json_mode).await
}

async fn run_local(conn: &ConnectionInfo, json_mode: bool) -> Result<()> {
    let url = format!("{}/health", conn.base_url);

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()?;
    let mut req = client.get(&url);
    if let Some(ref auth) = conn.auth_header {
        req = req.header("Authorization", auth);
    }
    let resp = match req.send().await {
        Ok(r) => r,
        Err(_) => {
            if json_mode {
                return output::print_error("App not running", json_mode);
            } else {
                eprintln!(
                    "{}\n  {}  Not running",
                    "Linkly AI Status".bold(),
                    "App:".dimmed()
                );
                anyhow::bail!("");
            }
        }
    };

    let status_code = resp.status().as_u16();
    if !(200..300).contains(&status_code) {
        let body = resp.text().await.unwrap_or_default();
        let body_trimmed = body.trim();
        if status_code == 401 {
            anyhow::bail!(
                "Authentication failed (401){}\n\
                 For LAN access: use --endpoint <url> --token <token>\n\
                 For remote access: run `linkly auth set-key <api-key>`",
                if body_trimmed.is_empty() { String::new() } else { format!(": {}", body_trimmed) }
            );
        }
        if body_trimmed.is_empty() {
            anyhow::bail!("Server error (HTTP {})", status_code);
        }
        anyhow::bail!("Server error (HTTP {}): {}", status_code, body_trimmed);
    }

    let health: HealthResponse = resp.json().await?;
    let version_gap = version_check::check_desktop_version(&health.version).err();

    if json_mode {
        // When the Desktop is older than this CLI requires, surface the
        // mismatch in `status` so a CI script keying off the JSON envelope
        // (`jq -e '.status == "success"'`) treats the run as a warning
        // rather than a clean pass — the connection is fine, but the
        // capability surface is incomplete.
        let envelope_status = if version_gap.is_some() { "warning" } else { "success" };
        let mut obj = serde_json::json!({
            "status": envelope_status,
            "cli_version": env!("CARGO_PKG_VERSION"),
            "app_version": health.version,
            "mcp_endpoint": health.mcp_endpoint,
            "doc_count": health.doc_count,
            "index_status": health.index_status,
        });
        if let Some(ref gap) = version_gap {
            obj["version_gap"] = serde_json::json!({
                "actual": gap.actual,
                "required": gap.required,
                "missing_features": gap.missing_features,
            });
        }
        println!("{}", obj);
    } else {
        let index_display = match health.index_status.as_str() {
            "watching" => "Up to date".green().to_string(),
            "scanning" => "Scanning...".yellow().to_string(),
            "indexing" => "Indexing...".yellow().to_string(),
            "idle" => "Idle".dimmed().to_string(),
            "error" => "Error".red().to_string(),
            other => other.to_string(),
        };

        println!("{}", "Linkly AI Status".bold());
        println!(
            "  {}  v{}",
            "CLI:".dimmed(),
            env!("CARGO_PKG_VERSION")
        );
        println!("  {}  v{}", "App:".dimmed(), health.version);
        if let Some(ref gap) = version_gap {
            // Indented under "App:" so it reads as an annotation on the
            // version line rather than a separate top-level field. Stays on
            // stdout (alongside the rest of the human-readable status block)
            // so a redirect like `linkly status > out.txt` keeps the warning
            // visible — we used to emit on stderr but that splits the report.
            println!(
                "        {} older than v{}: missing {}.",
                "⚠".yellow(),
                gap.required,
                gap.missing_features
            );
            println!(
                "          Update Desktop: open Settings → About → Check for Updates,"
            );
            println!("          or download from https://linkly.ai");
        }
        println!(
            "  {}  {}",
            "MCP:".dimmed(),
            health
                .mcp_endpoint
                .as_deref()
                .unwrap_or("not running")
        );
        println!(
            "  {} {} indexed",
            "Docs:".dimmed(),
            format_number(health.doc_count)
        );
        println!("  {} {}", "Index:".dimmed(), index_display);
    }

    Ok(())
}

async fn run_remote(conn: &ConnectionInfo, json_mode: bool) -> Result<()> {
    let url = format!("{}/mcp/health", conn.base_url);

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()?;
    let mut req = client.get(&url);
    if let Some(ref auth) = conn.auth_header {
        req = req.header("Authorization", auth);
    }
    let resp = match req.send().await {
        Ok(r) => r,
        Err(_) => {
            if json_mode {
                return output::print_error("Remote server unreachable", json_mode);
            } else {
                eprintln!(
                    "{}\n  {}  Unreachable",
                    "Linkly AI Remote Status".bold(),
                    "Server:".dimmed()
                );
                anyhow::bail!("");
            }
        }
    };

    let status_code = resp.status().as_u16();
    if status_code == 401 || status_code == 403 {
        let msg = format!(
            "Authentication failed ({}). Check your API key with `linkly auth set-key <api-key>`.",
            status_code
        );
        if json_mode {
            return output::print_error(&msg, json_mode);
        } else {
            eprintln!(
                "{}\n  {}  {}",
                "Linkly AI Remote Status".bold(),
                "Auth:".dimmed(),
                msg
            );
            anyhow::bail!("");
        }
    }
    if !(200..300).contains(&status_code) {
        let body = resp.text().await.unwrap_or_default();
        let body_trimmed = body.trim();
        if body_trimmed.is_empty() {
            anyhow::bail!("Server error (HTTP {})", status_code);
        }
        anyhow::bail!("Server error (HTTP {}): {}", status_code, body_trimmed);
    }

    let health: RemoteHealthResponse = resp.json().await?;
    let tunnel_status = health.tunnel.as_deref().unwrap_or("unknown");

    if json_mode {
        // Mirror the local-mode "warning vs success" envelope from C-14: a
        // disconnected tunnel means the upstream Desktop is unreachable
        // through this remote endpoint, which a CI script keying off the
        // JSON `status` field needs to treat as not-okay even though the
        // tunnel host itself responded.
        let envelope_status = if tunnel_status == "connected" {
            "success"
        } else {
            "warning"
        };
        let obj = serde_json::json!({
            "status": envelope_status,
            "mode": "remote",
            "cli_version": env!("CARGO_PKG_VERSION"),
            "server_status": health.status,
            "tunnel": tunnel_status,
        });
        println!("{}", obj);
    } else {
        let tunnel_display = match tunnel_status {
            "connected" => "Connected".green().to_string(),
            "disconnected" => "Disconnected".red().to_string(),
            other => other.yellow().to_string(),
        };

        println!("{}", "Linkly AI Remote Status".bold());
        println!(
            "  {}  v{}",
            "CLI:".dimmed(),
            env!("CARGO_PKG_VERSION")
        );
        println!("  {}  {}", "Server:".dimmed(), health.status);
        println!("  {}  {}", "Tunnel:".dimmed(), tunnel_display);
        println!("  {}  https://mcp.linkly.ai/mcp", "MCP:".dimmed());
    }

    Ok(())
}

fn format_number(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{},{:03}", n / 1_000, n % 1_000)
    } else {
        n.to_string()
    }
}
