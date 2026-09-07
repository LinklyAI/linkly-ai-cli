use anyhow::Result;
use owo_colors::OwoColorize;
use serde::Deserialize;

use crate::connection::{ConnectionInfo, ConnectionMode, RemoteHealthResponse};
use crate::output;
use crate::skills::{self, Local};
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

    // Started before the health request so the two round-trips overlap.
    let skill_check = tokio::spawn(check_skill());

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
            let message = unreachable_message(conn);
            if json_mode {
                println!(
                    "{}",
                    serde_json::json!({
                        "status": "error",
                        "code": "desktop_unreachable",
                        "message": message,
                        "endpoint": conn.base_url,
                    })
                );
                return Err(anyhow::Error::msg(""));
            } else {
                eprintln!(
                    "{}\n  {}  Unreachable\n  {}  {}",
                    "Linkly AI Status".bold(),
                    "App:".dimmed(),
                    "Endpoint:".dimmed(),
                    conn.base_url
                );
                eprintln!("\n{}", message);
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
                if body_trimmed.is_empty() {
                    String::new()
                } else {
                    format!(": {}", body_trimmed)
                }
            );
        }
        // A proxy in the path answers with its own status codes, so say so
        // rather than let the user read a 502 as "Linkly is broken". Returns
        // nothing for remote connections, where using the proxy is the point.
        let proxy_note = crate::connection::proxy_interception_note(conn)
            .map(|note| format!("\n\n{}", note))
            .unwrap_or_default();
        if body_trimmed.is_empty() {
            anyhow::bail!("Server error (HTTP {}){}", status_code, proxy_note);
        }
        anyhow::bail!(
            "Server error (HTTP {}): {}{}",
            status_code,
            body_trimmed,
            proxy_note
        );
    }

    let health: HealthResponse = resp.json().await?;
    let version_gap = version_check::check_desktop_version(&health.version).err();

    if json_mode {
        // When the Desktop is older than this CLI requires, surface the
        // mismatch in `status` so a CI script keying off the JSON envelope
        // (`jq -e '.status == "success"'`) treats the run as a warning
        // rather than a clean pass — the connection is fine, but the
        // capability surface is incomplete.
        let envelope_status = if version_gap.is_some() {
            "warning"
        } else {
            "success"
        };
        let mut obj = serde_json::json!({
            "status": envelope_status,
            "cli_version": env!("CARGO_PKG_VERSION"),
            "app_version": health.version,
            "mcp_endpoint": health.mcp_endpoint,
            "doc_count": health.doc_count,
            "index_status": health.index_status,
            "agent_skill": skill_json(&skill_check.await.unwrap_or_default()),
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
        println!("  {}  v{}", "CLI:".dimmed(), env!("CARGO_PKG_VERSION"));
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
            println!("          Update Desktop: open Settings → About → Check for Updates,");
            println!("          or download from https://linkly.ai");
        }
        println!(
            "  {}  {}",
            "MCP:".dimmed(),
            health.mcp_endpoint.as_deref().unwrap_or("not running")
        );
        println!(
            "  {} {} indexed",
            "Docs:".dimmed(),
            format_number(health.doc_count)
        );
        println!("  {} {}", "Index:".dimmed(), index_display);
        println!(
            "  {} {}",
            "Skill:".dimmed(),
            skill_display(&skill_check.await.unwrap_or_default())
        );
    }

    Ok(())
}

/// What `status` reports about the Agent Skill.
///
/// Read fresh on every run, with no throttling and no `LINKLY_NO_SKILLS_HINT`
/// opt-out: the tool-result notice is an interruption and has to be rationed,
/// but `status` is asked precisely because someone wants the current state.
#[derive(Default)]
struct SkillState {
    local: Option<Local>,
    latest: Option<semver::Version>,
}

async fn check_skill() -> SkillState {
    let local = skills::detect();
    // Offline is a normal state here; without it we simply cannot say whether
    // an installed copy is current, and we say so rather than guess.
    let latest = skills::fetch_latest().await.ok().map(|l| l.version);
    SkillState {
        local: Some(local),
        latest,
    }
}

/// A directory holding a working skill that the CLI will never write to, so
/// `status` names it — otherwise `update` appearing to do nothing is a mystery.
fn is_legacy(path: &std::path::Path) -> bool {
    skills::legacy_locations().iter().any(|p| p == path)
}

fn legacy_suffix(path: &std::path::Path) -> String {
    if is_legacy(path) {
        format!(" {}", format!("(legacy path: {})", path.display()).dimmed())
    } else {
        String::new()
    }
}

fn skill_display(state: &SkillState) -> String {
    let Some(ref local) = state.local else {
        return "unknown".dimmed().to_string();
    };
    match local {
        Local::Missing => format!(
            "{} — run `linkly skills install` ({})",
            "Not installed".yellow(),
            skills::DOCS_URL
        ),
        Local::Untracked(path) => format!(
            "{} — run `linkly skills update`{}",
            "version unknown".yellow(),
            legacy_suffix(path)
        ),
        Local::Unparseable(path) => {
            format!("{}{}", "version unrecognised".dimmed(), legacy_suffix(path))
        }
        Local::Tracked(path, current) => match &state.latest {
            Some(latest) if latest > current => format!(
                "v{} {}{}",
                current,
                format!("— v{} available, run `linkly skills update`", latest).yellow(),
                legacy_suffix(path)
            ),
            Some(_) => format!("v{}{}", current, legacy_suffix(path)),
            None => format!(
                "v{} {}{}",
                current,
                "(latest unknown)".dimmed(),
                legacy_suffix(path)
            ),
        },
    }
}

fn skill_json(state: &SkillState) -> serde_json::Value {
    let (installed, path) = match &state.local {
        None | Some(Local::Missing) => (None, None),
        Some(Local::Untracked(p)) => (Some("unknown".to_string()), Some(p)),
        Some(Local::Unparseable(p)) => (Some("unrecognised".to_string()), Some(p)),
        Some(Local::Tracked(p, v)) => (Some(v.to_string()), Some(p)),
    };
    serde_json::json!({
        "installed": installed,
        "latest": state.latest.as_ref().map(|v| v.to_string()),
        "path": path.map(|p| p.display().to_string()),
        "legacy_path": path.map(|p| is_legacy(p)).unwrap_or(false),
    })
}

fn unreachable_message(conn: &ConnectionInfo) -> String {
    let retry_failure = match &conn.mode {
        ConnectionMode::Local => "launch Linkly AI Desktop and try again",
        ConnectionMode::Lan { .. } => {
            "confirm Linkly AI Desktop is running on the target machine and try again"
        }
        ConnectionMode::Remote => "check your network connection and try again",
    };

    format!(
        "The CLI could not reach Linkly AI Desktop. This does not prove that Linkly AI Desktop is stopped.\n\
         If this command is running inside an AI-agent network sandbox:\n  \
           1. Use the Linkly MCP integration if it is configured.\n  \
           2. Otherwise, approve retrying this Linkly CLI command outside the network sandbox.\n\
         If that retry still fails, {}.\n{}",
        retry_failure,
        conn.doctor_hint()
    )
}

async fn run_remote(conn: &ConnectionInfo, json_mode: bool) -> Result<()> {
    let url = format!("{}/mcp/health", conn.base_url);

    // The skill is installed locally regardless of which endpoint we talk to,
    // so remote status reports it too.
    let skill_check = tokio::spawn(check_skill());

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
        // A proxy in the path answers with its own status codes, so say so
        // rather than let the user read a 502 as "Linkly is broken". Returns
        // nothing for remote connections, where using the proxy is the point.
        let proxy_note = crate::connection::proxy_interception_note(conn)
            .map(|note| format!("\n\n{}", note))
            .unwrap_or_default();
        if body_trimmed.is_empty() {
            anyhow::bail!("Server error (HTTP {}){}", status_code, proxy_note);
        }
        anyhow::bail!(
            "Server error (HTTP {}): {}{}",
            status_code,
            body_trimmed,
            proxy_note
        );
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
            "agent_skill": skill_json(&skill_check.await.unwrap_or_default()),
        });
        println!("{}", obj);
    } else {
        let tunnel_display = match tunnel_status {
            "connected" => "Connected".green().to_string(),
            "disconnected" => "Disconnected".red().to_string(),
            other => other.yellow().to_string(),
        };

        println!("{}", "Linkly AI Remote Status".bold());
        println!("  {}  v{}", "CLI:".dimmed(), env!("CARGO_PKG_VERSION"));
        println!("  {}  {}", "Server:".dimmed(), health.status);
        println!("  {}  {}", "Tunnel:".dimmed(), tunnel_display);
        println!("  {}  https://mcp.linkly.ai/mcp", "MCP:".dimmed());
        println!(
            "  {} {}",
            "Skill:".dimmed(),
            skill_display(&skill_check.await.unwrap_or_default())
        );
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
