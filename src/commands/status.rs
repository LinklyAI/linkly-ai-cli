use anyhow::Result;
use owo_colors::OwoColorize;
use serde::Deserialize;

use crate::connection::ConnectionInfo;
use crate::output;

#[derive(Deserialize)]
struct HealthResponse {
    version: String,
    doc_count: u64,
    mcp_endpoint: Option<String>,
    index_status: String,
}

pub async fn run(conn: &ConnectionInfo, json_mode: bool) -> Result<()> {
    let url = format!("{}/health", conn.base_url);

    let resp = match reqwest::get(&url).await {
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

fn format_number(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{},{:03}", n / 1_000, n % 1_000)
    } else {
        n.to_string()
    }
}
