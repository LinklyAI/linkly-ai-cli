use anyhow::Result;
use owo_colors::OwoColorize;
use serde::Deserialize;

use crate::connection::ConnectionInfo;
use crate::output;

/// Local desktop health response schema (GET /health)
#[derive(Deserialize)]
struct HealthResponse {
    version: String,
    doc_count: u64,
    mcp_endpoint: Option<String>,
    index_status: String,
}

/// Remote tunnel health response schema (GET /mcp/health)
#[derive(Deserialize)]
struct RemoteHealthResponse {
    status: String,
    tunnel: Option<String>,
}

pub async fn run(conn: &ConnectionInfo, json_mode: bool) -> Result<()> {
    if conn.is_remote {
        return run_remote(conn, json_mode).await;
    }
    run_local(conn, json_mode).await
}

async fn run_local(conn: &ConnectionInfo, json_mode: bool) -> Result<()> {
    let url = format!("{}/health", conn.base_url);

    let client = reqwest::Client::new();
    let mut req = client.get(&url);
    if let Some(ref auth) = conn.auth_header {
        req = req.header("Authorization", auth);
    }
    let resp = match req.send().await {
        Ok(r) => r,
        Err(_) => {
            if json_mode {
                output::print_error("App not running", json_mode);
            } else {
                eprintln!(
                    "{}\n  {}  Not running",
                    "Linkly AI Status".bold(),
                    "App:".dimmed()
                );
            }
            return Ok(());
        }
    };

    let health: HealthResponse = resp.json().await?;

    if json_mode {
        let obj = serde_json::json!({
            "status": "success",
            "cli_version": env!("CARGO_PKG_VERSION"),
            "app_version": health.version,
            "mcp_endpoint": health.mcp_endpoint,
            "doc_count": health.doc_count,
            "index_status": health.index_status,
        });
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
                output::print_error("Remote server unreachable", json_mode);
            } else {
                eprintln!(
                    "{}\n  {}  Unreachable",
                    "Linkly AI Remote Status".bold(),
                    "Server:".dimmed()
                );
            }
            return Ok(());
        }
    };

    let status_code = resp.status().as_u16();
    if status_code == 401 || status_code == 403 {
        if json_mode {
            output::print_error(&format!("Authentication failed ({})", status_code), json_mode);
        } else {
            eprintln!(
                "{}\n  {}  Authentication failed ({})",
                "Linkly AI Remote Status".bold(),
                "Auth:".dimmed(),
                status_code
            );
        }
        return Ok(());
    }

    let health: RemoteHealthResponse = resp.json().await?;
    let tunnel_status = health.tunnel.as_deref().unwrap_or("unknown");

    if json_mode {
        let obj = serde_json::json!({
            "status": "success",
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
