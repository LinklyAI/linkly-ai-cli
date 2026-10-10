use std::time::{Duration, Instant};

use anyhow::Result;
use owo_colors::OwoColorize;
use serde::Deserialize;

use crate::cli::{ConnectionArgs, COMMUNITY_URL};
use crate::client::McpClient;
use crate::commands::auth::{mask_key, mask_lan_token, remote_key_origin};
use crate::connection::{self, ConnectionInfo, ConnectionMode, RemoteHealthResponse};

/// Individual check result.
struct Check {
    name: &'static str,
    ok: bool,
    detail: String,
    latency_ms: Option<u64>,
    advice: Option<String>,
}

/// Local / LAN health response.
#[derive(Deserialize)]
struct LocalHealthResponse {
    version: String,
    doc_count: u64,
    index_status: String,
}

/// Entry point from main.
///
/// With a connection flag (`--remote`, `--lan`, `--endpoint`) only that
/// connection is diagnosed, exactly as before. Without one, every connection
/// this machine can use is diagnosed at once and reported in groups: the
/// local Desktop always, the remote gateway and the LAN Desktop when
/// credentials for them are saved. The run succeeds when at least one group
/// is usable — an agent on a server with no Desktop but a working remote
/// connection is healthy, not broken.
pub async fn run_from_args(
    args: &ConnectionArgs,
    client: Option<&str>,
    json_mode: bool,
) -> Result<()> {
    if args.remote || args.lan || args.endpoint.is_some() {
        let (mode, checks) = diagnose(
            args.endpoint.as_deref(),
            args.token.as_deref(),
            args.lan,
            args.remote,
            client,
        )
        .await;
        let (mode_name, title) = match &mode {
            ConnectionMode::Local => ("local", "Local (this machine)"),
            ConnectionMode::Remote => ("remote", "Remote (--remote)"),
            ConnectionMode::Lan { saved: true, .. } => ("lan", "LAN (--lan)"),
            ConnectionMode::Lan { .. } => ("lan", "LAN (--endpoint)"),
        };
        let group = Group {
            mode: mode_name,
            title,
            checks: Some(checks),
            setup_hint: "",
        };
        if json_mode {
            // Same envelope as the flagless run, plus the single-connection
            // fields this command printed before groups existed, kept at the
            // top level for existing consumers.
            let mut obj = groups_json(std::slice::from_ref(&group));
            if let (Some(map), Some(checks)) = (obj.as_object_mut(), group.checks.as_ref()) {
                if let serde_json::Value::Object(legacy) = checks_json(checks, &mode) {
                    for (key, value) in legacy {
                        map.entry(key).or_insert(value);
                    }
                }
            }
            println!("{}", obj);
        } else {
            print_groups_human(std::slice::from_ref(&group));
        }
        if !group.usable() {
            // Empty message: the printed checks already explained.
            anyhow::bail!("");
        }
        return Ok(());
    }
    run_all(client, json_mode).await
}

/// Resolve one connection and run its checks. A resolution failure (no port
/// file, no saved credentials) becomes a single failed "Connection" check.
async fn diagnose(
    endpoint: Option<&str>,
    token: Option<&str>,
    lan: bool,
    remote: bool,
    client: Option<&str>,
) -> (ConnectionMode, Vec<Check>) {
    let mode = if remote {
        ConnectionMode::Remote
    } else if endpoint.is_some() || lan {
        ConnectionMode::Lan {
            endpoint: endpoint.unwrap_or_default().to_string(),
            saved: lan,
        }
    } else {
        ConnectionMode::Local
    };

    match connection::resolve(endpoint, token, lan, remote, client) {
        Ok(conn) => {
            let checks = match &conn.mode {
                ConnectionMode::Local => run_local(&conn).await,
                ConnectionMode::Lan { .. } => run_lan(&conn).await,
                ConnectionMode::Remote => run_remote(&conn).await,
            };
            (conn.mode, checks)
        }
        Err(e) => {
            let advice = match &mode {
                ConnectionMode::Local => {
                    "Launch Linkly AI Desktop on this machine, or use --remote / --lan."
                }
                ConnectionMode::Remote => {
                    "Run 'linkly auth login' to sign in.\n    Or paste a key from https://linkly.ai (Dashboard > API Keys): linkly auth set-key <your-api-key>"
                }
                ConnectionMode::Lan { saved: true, .. } => {
                    "Run 'linkly auth set-key <token> --lan --endpoint <url>' to save your LAN connection.\n    Find the address and token in Linkly AI Desktop > Settings > MCP."
                }
                ConnectionMode::Lan { .. } => "Check --endpoint and --token values.",
            };
            // The error's first line states the problem; its how-to-connect
            // paragraph would repeat what the advice and the other groups say.
            let message = e.to_string();
            let checks = vec![Check {
                name: "Connection",
                ok: false,
                detail: message.lines().next().unwrap_or_default().to_string(),
                latency_ms: None,
                advice: Some(advice.to_string()),
            }];
            (mode, checks)
        }
    }
}

/// Failed checks that make a connection unusable. "Version" is informational:
/// an outdated Desktop is worth knowing but doesn't block anything.
fn hard_failures(checks: &[Check]) -> usize {
    checks
        .iter()
        .filter(|c| !c.ok && c.name != "Version")
        .count()
}

/// One connection in the all-connections report.
struct Group {
    /// "local" / "remote" / "lan" — the JSON `mode`.
    mode: &'static str,
    /// Human heading, naming the flag that selects this connection.
    title: &'static str,
    /// `None` when there are no credentials for it: shown, not diagnosed.
    checks: Option<Vec<Check>>,
    /// What to run to set it up, for the not-configured case.
    setup_hint: &'static str,
}

impl Group {
    fn status(&self) -> &'static str {
        match &self.checks {
            None => "not_configured",
            Some(checks) if hard_failures(checks) > 0 => "error",
            Some(checks) if checks.iter().any(|c| !c.ok) => "warning",
            Some(_) => "ok",
        }
    }

    fn usable(&self) -> bool {
        matches!(self.status(), "ok" | "warning")
    }
}

async fn run_all(client: Option<&str>, json_mode: bool) -> Result<()> {
    // A corrupted credentials file counts as "configured": diagnosing it
    // surfaces the error instead of hiding it behind "not configured".
    let remote_saved = !matches!(connection::read_credentials_api_key(), Ok(None));
    let lan_saved = !matches!(connection::read_lan_credentials(), Ok(None));

    // Run concurrently: the slowest connection sets the total time.
    let (local, remote, lan) = tokio::join!(
        diagnose(None, None, false, false, client),
        async {
            if remote_saved {
                Some(diagnose(None, None, false, true, client).await.1)
            } else {
                None
            }
        },
        async {
            if lan_saved {
                Some(diagnose(None, None, true, false, client).await.1)
            } else {
                None
            }
        },
    );

    let groups = [
        Group {
            mode: "local",
            title: "Local (this machine)",
            checks: Some(local.1),
            setup_hint: "",
        },
        Group {
            mode: "remote",
            title: "Remote (--remote)",
            checks: remote,
            setup_hint: "Not configured. Run `linkly auth login` to enable --remote.",
        },
        Group {
            mode: "lan",
            title: "LAN (--lan)",
            checks: lan,
            setup_hint:
                "Not configured. Run `linkly auth set-key <token> --lan --endpoint <url>` to enable --lan.",
        },
    ];

    if json_mode {
        println!("{}", groups_json(&groups));
    } else {
        print_groups_human(&groups);
    }

    if !groups.iter().any(Group::usable) {
        anyhow::bail!("");
    }
    Ok(())
}

// ── Local mode checks ────────────────────────────────────

async fn run_local(conn: &ConnectionInfo) -> Vec<Check> {
    let mut checks = Vec::new();

    // 1. Port file (already validated by resolve(), so if we're here it's ok)
    checks.push(Check {
        name: "Port file",
        ok: true,
        detail: "~/.linkly/port readable".to_string(),
        latency_ms: None,
        advice: None,
    });

    // 2. HTTP connectivity + 3. App status
    run_health_check(&mut checks, conn, false).await;

    // 4. MCP round-trip — Desktop's HTTP /health endpoint can be alive
    // while the MCP server is broken (initialize handshake hangs, tools/list
    // fails, transport mismatch). Without this check `linkly doctor` would
    // report "all green" yet every subsequent tool call fails. Mirrors the
    // remote-mode flow so all three connection modes have the same
    // diagnostic depth.
    let all_ok = checks.iter().all(|c| c.ok);
    if all_ok {
        run_mcp_roundtrip(&mut checks, conn).await;
    }

    checks
}

// ── LAN mode checks ─────────────────────────────────────

async fn run_lan(conn: &ConnectionInfo) -> Vec<Check> {
    let mut checks = Vec::new();

    // 0. Credentials — which address and token are in play.
    let token_preview = conn
        .auth_header
        .as_ref()
        .and_then(|h| h.strip_prefix("Bearer "))
        .map(mask_lan_token)
        .unwrap_or_default();
    let detail = match &conn.mode {
        ConnectionMode::Lan { saved: true, .. } => format!(
            "LAN connection saved ({}, token {})",
            conn.base_url, token_preview
        ),
        _ => format!(
            "Using --endpoint and --token ({}, token {})",
            conn.base_url, token_preview
        ),
    };
    checks.push(Check {
        name: "Credentials",
        ok: true,
        detail,
        latency_ms: None,
        advice: None,
    });

    // 1. HTTP connectivity + 2. Auth + 3. App status
    run_health_check(&mut checks, conn, false).await;

    // 4. MCP round-trip (same rationale as `run_local`).
    let all_ok = checks.iter().all(|c| c.ok);
    if all_ok {
        run_mcp_roundtrip(&mut checks, conn).await;
    }

    checks
}

// ── Remote mode checks ──────────────────────────────────

async fn run_remote(conn: &ConnectionInfo) -> Vec<Check> {
    let mut checks = Vec::new();

    // 1. Credentials — same masking and wording as `linkly auth status`.
    let key_preview = conn
        .auth_header
        .as_ref()
        .and_then(|h| h.strip_prefix("Bearer "))
        .map(mask_key)
        .unwrap_or_default();
    checks.push(Check {
        name: "Credentials",
        ok: true,
        detail: format!(
            "API key configured ({}, {})",
            key_preview,
            remote_key_origin()
        ),
        latency_ms: None,
        advice: None,
    });

    // 2. Server reachability + 3. Auth + 4. Tunnel status
    run_health_check(&mut checks, conn, true).await;

    // 5. MCP round-trip (only if all previous checks passed)
    let all_ok = checks.iter().all(|c| c.ok);
    if all_ok {
        run_mcp_roundtrip(&mut checks, conn).await;
    }

    checks
}

// ── Shared health check ─────────────────────────────────

async fn run_health_check(checks: &mut Vec<Check>, conn: &ConnectionInfo, is_remote: bool) {
    let health_url = if is_remote {
        format!("{}/mcp/health", conn.base_url)
    } else {
        format!("{}/health", conn.base_url)
    };

    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            checks.push(Check {
                name: "Server",
                ok: false,
                detail: format!("HTTP client error: {}", e),
                latency_ms: None,
                advice: None,
            });
            return;
        }
    };

    let mut req = client.get(&health_url);
    if let Some(header) = &conn.auth_header {
        req = req.header("Authorization", header);
    }

    let start = Instant::now();
    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            let detail = if e.is_connect() {
                format!("{} unreachable", conn.base_url)
            } else if e.is_timeout() {
                format!("{} timed out", conn.base_url)
            } else {
                format!("Connection error: {}", e)
            };
            let base_advice = match &conn.mode {
                ConnectionMode::Local => "The CLI could not reach Linkly AI Desktop. This does not prove that Linkly AI Desktop is stopped.\n    If this command is running inside an AI-agent network sandbox:\n      1. Use the Linkly MCP integration if it is configured.\n      2. Otherwise, approve retrying this Linkly CLI command outside the network sandbox.\n    If that retry still fails, launch Linkly AI Desktop and try again."
                    .to_string(),
                ConnectionMode::Lan { .. } => "The CLI could not reach the configured Linkly AI Desktop endpoint. Check that the endpoint is correct, but note that this does not prove that Linkly AI Desktop is stopped.\n    If this command is running inside an AI-agent network sandbox:\n      1. Use the Linkly MCP integration if it is configured.\n      2. Otherwise, approve retrying this Linkly CLI command outside the network sandbox.\n    If that retry still fails, confirm Linkly AI Desktop is running on the target machine."
                    .to_string(),
                ConnectionMode::Remote => "Check your network connection.".to_string(),
            };
            // `doctor` exists to point at the real cause; a proxy silently in
            // the path is exactly the case where the raw error misleads.
            let advice = Some(match crate::connection::proxy_interception_note(conn) {
                Some(note) => format!("{}\n    {}", base_advice, note.replace('\n', "\n    ")),
                None => base_advice,
            });
            checks.push(Check {
                name: "Server",
                ok: false,
                detail,
                latency_ms: None,
                advice,
            });
            return;
        }
    };
    let latency = start.elapsed().as_millis() as u64;

    let status = resp.status().as_u16();

    // Auth check
    if status == 401 || status == 403 {
        let body = resp.text().await.unwrap_or_default();
        let (detail, advice) = match status {
            401 => {
                let advice = match &conn.mode {
                    ConnectionMode::Lan { saved: true, .. } => {
                        "The saved LAN token was rejected. Copy the current token from Desktop > Settings > MCP\n    and run 'linkly auth set-key <token> --lan --endpoint <url>'."
                    }
                    ConnectionMode::Lan { .. } => {
                        "Check your --token value (find it in Desktop > Settings > MCP)."
                    }
                    ConnectionMode::Remote => {
                        "Your API key was rejected. Run 'linkly auth logout', then 'linkly auth login'\n    (or 'linkly auth set-key <your-api-key>' with a key from https://linkly.ai Dashboard > API Keys)."
                    }
                    _ => "Unexpected auth failure for local connection.",
                };
                (
                    format!("Authentication failed (401): {}", body.trim()),
                    advice.to_string(),
                )
            }
            _ => {
                let advice = if body.contains("disabled") {
                    "Ask the Desktop owner to enable LAN access: Settings > MCP > LAN Access."
                } else {
                    "Check your credentials and permissions."
                };
                (
                    format!("Access denied (403): {}", body.trim()),
                    advice.to_string(),
                )
            }
        };
        // Server is reachable
        checks.push(Check {
            name: "Server",
            ok: true,
            detail: format!("{} reachable ({}ms)", conn.base_url, latency),
            latency_ms: Some(latency),
            advice: None,
        });
        checks.push(Check {
            name: "Auth",
            ok: false,
            detail,
            latency_ms: None,
            advice: Some(advice),
        });
        return;
    }

    if !(200..300).contains(&status) {
        let body = resp.text().await.unwrap_or_default();
        checks.push(Check {
            name: "Server",
            ok: false,
            detail: match crate::connection::proxy_interception_note(conn) {
                Some(note) => format!(
                    "HTTP {}: {} — {}",
                    status,
                    body.trim(),
                    note.lines().next().unwrap_or_default().trim()
                ),
                None => format!("HTTP {}: {}", status, body.trim()),
            },
            latency_ms: Some(latency),
            advice: None,
        });
        return;
    }

    // Server OK
    let host_label = if is_remote {
        "mcp.linkly.ai".to_string()
    } else {
        conn.base_url.clone()
    };
    checks.push(Check {
        name: "Server",
        ok: true,
        detail: format!("{} reachable ({}ms)", host_label, latency),
        latency_ms: Some(latency),
        advice: None,
    });

    // Auth OK (implicit for 2xx)
    if conn.auth_header.is_some() {
        checks.push(Check {
            name: "Auth",
            ok: true,
            detail: "Authenticated".to_string(),
            latency_ms: None,
            advice: None,
        });
    }

    // Parse response body
    let body = resp.text().await.unwrap_or_default();

    if is_remote {
        // Remote: check tunnel status
        if let Ok(health) = serde_json::from_str::<RemoteHealthResponse>(&body) {
            let tunnel_status = health.tunnel.as_deref().unwrap_or("unknown");
            let ok = tunnel_status == "connected";
            checks.push(Check {
                name: "Tunnel",
                ok,
                detail: if ok {
                    "Desktop is connected".to_string()
                } else {
                    "Desktop is disconnected".to_string()
                },
                latency_ms: None,
                advice: if ok {
                    None
                } else {
                    Some(
                        "Launch Linkly AI Desktop and enable \"MCP Connector\" in Settings > MCP."
                            .to_string(),
                    )
                },
            });
        }
    } else {
        // Local/LAN: parse app status
        if let Ok(health) = serde_json::from_str::<LocalHealthResponse>(&body) {
            checks.push(Check {
                name: "App",
                ok: true,
                detail: format!(
                    "v{}, {} docs, index: {}",
                    health.version, health.doc_count, health.index_status
                ),
                latency_ms: None,
                advice: None,
            });

            // Compatibility check is split out from the connectivity-focused
            // App check so the user can tell "Desktop is reachable" apart
            // from "Desktop has the features this CLI expects". A non-SemVer
            // app version (dev build, …) makes `check_desktop_version` return
            // Ok and we skip pushing a Version check entirely — false alarms
            // would be more annoying than the missing reassurance.
            match crate::version_check::check_desktop_version(&health.version) {
                Ok(()) => {
                    if semver::Version::parse(&health.version).is_ok() {
                        checks.push(Check {
                            name: "Version",
                            ok: true,
                            detail: format!(
                                "Desktop v{} meets CLI requirements (≥ v{})",
                                health.version,
                                crate::version_check::MIN_DESKTOP_VERSION_FOR_FULL_FEATURES
                                    .trim_end_matches("-beta.0")
                            ),
                            latency_ms: None,
                            advice: None,
                        });
                    }
                }
                Err(gap) => {
                    checks.push(Check {
                        name: "Version",
                        ok: false,
                        detail: format!(
                            "Desktop v{} is older than v{} (missing {})",
                            gap.actual, gap.required, gap.missing_features
                        ),
                        latency_ms: None,
                        advice: Some(format!(
                            "Update Desktop to v{} or later. Open Linkly AI Desktop > Settings > About > Check for Updates,\n    or download the latest installer from https://linkly.ai.",
                            gap.required
                        )),
                    });
                }
            }
        }
    }
}

// ── MCP round-trip check ────────────────────────────────

async fn run_mcp_roundtrip(checks: &mut Vec<Check>, conn: &ConnectionInfo) {
    let start = Instant::now();
    // 10s outer timeout overrides the per-operation timeouts inside connect (30s) / call_tool (60s),
    // since doctor should give a quick verdict rather than making the user wait.
    // Uses tools/list (MCP built-in) instead of calling a specific tool, so it works
    // regardless of which tools the Desktop registers.
    let result = tokio::time::timeout(Duration::from_secs(10), async {
        let client = McpClient::connect(conn).await?;
        client.list_tools().await
    })
    .await;

    let latency = start.elapsed().as_millis() as u64;

    match result {
        Ok(Ok(tools)) => {
            checks.push(Check {
                name: "MCP",
                ok: true,
                detail: format!(
                    "Round-trip OK, {} tools available ({}ms)",
                    tools.len(),
                    latency
                ),
                latency_ms: Some(latency),
                advice: None,
            });
        }
        Ok(Err(e)) => {
            checks.push(Check {
                name: "MCP",
                ok: false,
                detail: format!("MCP session failed: {}", e),
                latency_ms: Some(latency),
                advice: Some(
                    "The tunnel is connected but the Desktop MCP service may be unresponsive.\n    Try restarting Linkly AI Desktop."
                        .to_string(),
                ),
            });
        }
        Err(_) => {
            checks.push(Check {
                name: "MCP",
                ok: false,
                detail: "Round-trip timed out (10s)".to_string(),
                latency_ms: Some(latency),
                advice: Some(
                    "The tunnel is connected but the Desktop is not responding.\n    Try restarting Linkly AI Desktop."
                        .to_string(),
                ),
            });
        }
    }
}

// ── Output formatters ───────────────────────────────────

/// The check lines of one connection, followed by advice for failed ones.
fn print_checks(checks: &[Check]) {
    let max_name_len = checks.iter().map(|c| c.name.len()).max().unwrap_or(0);

    for check in checks {
        let label = if check.ok {
            "[ok]".green().to_string()
        } else {
            "[FAIL]".red().to_string()
        };
        println!(
            "  {:width$}  {} {}",
            format!("{}:", check.name).dimmed(),
            label,
            check.detail,
            width = max_name_len + 1
        );
    }

    let advices: Vec<&str> = checks
        .iter()
        .filter(|c| !c.ok)
        .filter_map(|c| c.advice.as_deref())
        .collect();
    if !advices.is_empty() {
        println!();
        for advice in &advices {
            println!("  {}", advice);
        }
    }
}

fn print_groups_human(groups: &[Group]) {
    println!("{}", "Linkly AI Doctor".bold());
    for group in groups {
        println!();
        println!("{}", group.title.bold());
        match &group.checks {
            Some(checks) => print_checks(checks),
            None => println!("  {}", group.setup_hint.dimmed()),
        }
    }

    println!();
    let summary: Vec<String> = groups
        .iter()
        .map(|g| {
            let state = match g.status() {
                "ok" => "ok".green().to_string(),
                "warning" => "ok, with warnings".yellow().to_string(),
                "error" => "failed".red().to_string(),
                _ => "not configured".dimmed().to_string(),
            };
            let name = if g.mode == "lan" { "LAN" } else { g.mode };
            format!("{} {}", name, state)
        })
        .collect();
    println!("{} {}", "Summary:".bold(), summary.join(", "));
    if groups.iter().any(|g| g.status() == "error") {
        println!("Still stuck? Ask the community: {}", COMMUNITY_URL);
    }
}

fn groups_json(groups: &[Group]) -> serde_json::Value {
    let usable = groups.iter().filter(|g| g.usable()).count();
    let clean = groups
        .iter()
        .filter(|g| g.checks.is_some())
        .all(|g| g.status() == "ok");
    let status = if usable == 0 {
        "error"
    } else if clean {
        "ok"
    } else {
        "warning"
    };
    let groups_json: Vec<serde_json::Value> = groups
        .iter()
        .map(|g| match &g.checks {
            Some(checks) => {
                let mut obj = checks_json_for(checks, g.mode);
                obj["configured"] = serde_json::json!(true);
                obj
            }
            None => serde_json::json!({
                "mode": g.mode,
                "configured": false,
                "status": "not_configured",
                "hint": g.setup_hint,
            }),
        })
        .collect();
    serde_json::json!({ "status": status, "groups": groups_json })
}

/// The single-connection JSON envelope (`doctor --remote --json` etc.).
fn checks_json(checks: &[Check], mode: &ConnectionMode) -> serde_json::Value {
    let mode = match mode {
        ConnectionMode::Local => "local",
        ConnectionMode::Lan { .. } => "lan",
        ConnectionMode::Remote => "remote",
    };
    checks_json_for(checks, mode)
}

fn checks_json_for(checks: &[Check], mode: &str) -> serde_json::Value {
    // Split failed checks into hard failures and warnings. The "Version"
    // check is informational only — an outdated Desktop is a real concern
    // but doesn't block diagnostics from running, and `run()` deliberately
    // keeps it out of the exit-code calculation. The JSON envelope must
    // use the same partition so a CI script that keys off `.status` and
    // one that keys off `$?` reach the same conclusion.
    let hard_failures = checks
        .iter()
        .filter(|c| !c.ok && c.name != "Version")
        .count();
    let warnings = checks
        .iter()
        .filter(|c| !c.ok && c.name == "Version")
        .count();
    let total_issues = hard_failures + warnings;

    let envelope_status = if hard_failures > 0 {
        "error"
    } else if warnings > 0 {
        "warning"
    } else {
        "ok"
    };

    let checks_json: Vec<serde_json::Value> = checks
        .iter()
        .map(|c| {
            let mut obj = serde_json::json!({
                "name": c.name,
                "ok": c.ok,
                "detail": c.detail,
            });
            if let Some(ms) = c.latency_ms {
                obj["latency_ms"] = serde_json::json!(ms);
            }
            if let Some(advice) = &c.advice {
                obj["advice"] = serde_json::json!(advice);
            }
            obj
        })
        .collect();

    // Collect advice from all failed checks
    let advice: String = checks
        .iter()
        .filter(|c| !c.ok && c.advice.is_some())
        .map(|c| c.advice.as_deref().unwrap())
        .collect::<Vec<_>>()
        .join("\n");

    serde_json::json!({
        "status": envelope_status,
        "mode": mode,
        "checks": checks_json,
        "issues": total_issues,
        "hard_failures": hard_failures,
        "warnings": warnings,
        "advice": if advice.is_empty() { serde_json::Value::Null } else { serde_json::json!(advice) },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(name: &'static str, ok: bool) -> Check {
        Check {
            name,
            ok,
            detail: String::new(),
            latency_ms: None,
            advice: None,
        }
    }

    fn group(checks: Option<Vec<Check>>) -> Group {
        Group {
            mode: "remote",
            title: "Remote (--remote)",
            checks,
            setup_hint: "",
        }
    }

    #[test]
    fn group_status_separates_failures_warnings_and_unconfigured() {
        assert_eq!(group(None).status(), "not_configured");
        assert_eq!(group(Some(vec![check("Server", true)])).status(), "ok");
        assert_eq!(
            group(Some(vec![check("Server", true), check("Version", false)])).status(),
            "warning"
        );
        assert_eq!(
            group(Some(vec![check("Server", false), check("Version", false)])).status(),
            "error"
        );
    }

    #[test]
    fn only_diagnosed_groups_without_hard_failures_are_usable() {
        assert!(!group(None).usable());
        assert!(group(Some(vec![check("Version", false)])).usable());
        assert!(!group(Some(vec![check("Connection", false)])).usable());
    }

    #[test]
    fn local_without_a_desktop_reports_a_connection_failure() {
        // No port file in a fresh HOME: resolution fails before any network.
        crate::test_helpers::with_temp_home("doctor_no_desktop", |_home| {
            let (mode, checks) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(diagnose(None, None, false, false, None));
            assert!(matches!(mode, ConnectionMode::Local));
            assert_eq!(checks.len(), 1);
            assert_eq!(checks[0].name, "Connection");
            assert!(!checks[0].ok);
        });
    }
}
