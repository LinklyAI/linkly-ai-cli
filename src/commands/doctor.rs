use std::time::{Duration, Instant};

use anyhow::Result;
use owo_colors::OwoColorize;
use serde::Deserialize;

use crate::cli::ConnectionArgs;
use crate::client::McpClient;
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

/// Entry point from main — handles connection resolution failures gracefully.
pub async fn run_from_args(args: &ConnectionArgs, json_mode: bool) -> Result<()> {
    // Determine mode from args (before resolve, which may fail)
    let mode = if args.remote {
        ConnectionMode::Remote
    } else if args.endpoint.is_some() {
        ConnectionMode::Lan {
            endpoint: args.endpoint.clone().unwrap_or_default(),
        }
    } else {
        ConnectionMode::Local
    };

    match connection::resolve(args.endpoint.as_deref(), args.token.as_deref(), args.remote) {
        Ok(conn) => run(&conn, json_mode).await,
        Err(e) => {
            // resolve() failed — report it as the first check failure, then stop
            let checks = vec![Check {
                name: "Connection",
                ok: false,
                detail: format!("{}", e),
                latency_ms: None,
                advice: match &mode {
                    ConnectionMode::Local => Some(
                        "Launch Linkly AI Desktop on this machine, or use --remote / --endpoint."
                            .to_string(),
                    ),
                    ConnectionMode::Remote => Some(
                        "Run 'linkly auth set-key <your-api-key>' to configure.\n    Get your API key from https://linkly.ai (Dashboard > API Keys)."
                            .to_string(),
                    ),
                    ConnectionMode::Lan { .. } => Some(
                        "Check --endpoint and --token values.".to_string(),
                    ),
                },
            }];

            let dummy_conn = ConnectionInfo {
                mcp_url: String::new(),
                base_url: String::new(),
                auth_header: None,
                is_remote: args.remote,
                mode,
            };

            if json_mode {
                print_json(&checks, &dummy_conn);
            } else {
                print_human(&checks);
            }
            Ok(())
        }
    }
}

pub async fn run(conn: &ConnectionInfo, json_mode: bool) -> Result<()> {
    let checks = match &conn.mode {
        ConnectionMode::Local => run_local(conn).await,
        ConnectionMode::Lan { .. } => run_lan(conn).await,
        ConnectionMode::Remote => run_remote(conn).await,
    };

    if json_mode {
        print_json(&checks, conn);
    } else {
        print_human(&checks);
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

    checks
}

// ── LAN mode checks ─────────────────────────────────────

async fn run_lan(conn: &ConnectionInfo) -> Vec<Check> {
    let mut checks = Vec::new();

    // 1. HTTP connectivity + 2. Auth + 3. App status
    run_health_check(&mut checks, conn, false).await;

    checks
}

// ── Remote mode checks ──────────────────────────────────

async fn run_remote(conn: &ConnectionInfo) -> Vec<Check> {
    let mut checks = Vec::new();

    // 1. Credentials
    let key_preview = conn
        .auth_header
        .as_ref()
        .and_then(|h| h.strip_prefix("Bearer "))
        .map(|k| {
            if k.len() > 10 {
                format!("{}...{}", &k[..6], &k[k.len() - 4..])
            } else {
                k.to_string()
            }
        })
        .unwrap_or_default();
    checks.push(Check {
        name: "Credentials",
        ok: true,
        detail: format!("API key configured ({})", key_preview),
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
            let advice = match &conn.mode {
                ConnectionMode::Local => {
                    Some("Make sure Linkly AI Desktop is running on this machine.".to_string())
                }
                ConnectionMode::Lan { .. } => Some(
                    "Check that the endpoint is correct and Desktop is running on the target machine.".to_string(),
                ),
                ConnectionMode::Remote => {
                    Some("Check your network connection.".to_string())
                }
            };
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
                    ConnectionMode::Lan { .. } => {
                        "Check your --token value (find it in Desktop > Settings > MCP)."
                    }
                    ConnectionMode::Remote => {
                        "Check your API key: run 'linkly auth set-key <your-api-key>'.\n    Get your key from https://linkly.ai (Dashboard > API Keys)."
                    }
                    _ => "Unexpected auth failure for local connection.",
                };
                (format!("Authentication failed (401): {}", body.trim()), advice.to_string())
            }
            _ => {
                let advice = if body.contains("disabled") {
                    "Ask the Desktop owner to enable LAN access: Settings > MCP > LAN Access."
                } else {
                    "Check your credentials and permissions."
                };
                (format!("Access denied (403): {}", body.trim()), advice.to_string())
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
            detail: format!("HTTP {}: {}", status, body.trim()),
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
                detail: format!("Round-trip OK, {} tools available ({}ms)", tools.len(), latency),
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

fn print_human(checks: &[Check]) {
    println!("{}", "Linkly AI Doctor".bold());
    println!();

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

    // Collect advice from failed checks
    let advices: Vec<&str> = checks
        .iter()
        .filter(|c| !c.ok && c.advice.is_some())
        .map(|c| c.advice.as_deref().unwrap())
        .collect();

    if !advices.is_empty() {
        println!();
        for advice in &advices {
            println!("  {}", advice);
        }
    }

    let issues = checks.iter().filter(|c| !c.ok).count();
    println!();
    if issues == 0 {
        println!("  {}", "All checks passed.".green());
    } else {
        println!(
            "  {}",
            format!("{} issue{} found.", issues, if issues == 1 { "" } else { "s" }).red()
        );
    }
}

fn print_json(checks: &[Check], conn: &ConnectionInfo) {
    let mode = match &conn.mode {
        ConnectionMode::Local => "local",
        ConnectionMode::Lan { .. } => "lan",
        ConnectionMode::Remote => "remote",
    };

    let issues = checks.iter().filter(|c| !c.ok).count();

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

    let output = serde_json::json!({
        "status": if issues == 0 { "ok" } else { "error" },
        "mode": mode,
        "checks": checks_json,
        "issues": issues,
        "advice": if advice.is_empty() { serde_json::Value::Null } else { serde_json::json!(advice) },
    });

    println!("{}", output);
}
