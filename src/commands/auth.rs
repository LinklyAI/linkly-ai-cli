use anyhow::{bail, Result};
use owo_colors::OwoColorize;
use serde::Deserialize;

use crate::connection::{
    credentials_path, delete_lan_credentials, delete_remote_credentials, normalize_endpoint,
    read_credentials_api_key, read_lan_credentials, save_credentials_api_key, save_lan_credentials,
    LanCredentials,
};
use crate::output;

/// Endpoint used to validate a stored key and resolve its plan. It is a
/// read-only quota probe — it does not consume quota — and it is the only
/// gateway route that echoes the caller's plan.
const AUTH_STATUS_URL: &str = "https://mcp.linkly.ai/v1/rate-limit-status";

/// Subset of `/v1/rate-limit-status` we care about here.
#[derive(Deserialize)]
struct AuthStatusResponse {
    plan: Option<String>,
}

/// Which stored credential an `auth` subcommand acts on. Mirrors the
/// `--remote` / `--lan` connection flags; `Remote` is the default because it is
/// what `auth` meant before LAN credentials could be saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialTarget {
    Remote,
    Lan,
}

/// Handle `linkly auth set-key <key>` (remote, the default)
pub fn set_key(key: &str) -> Result<()> {
    // Validate format: lkai_ prefix + 32 hex chars
    if !key.starts_with("lkai_") || key.len() != 37 {
        bail!(
            "Invalid API key format. Expected: lkai_<32 hex chars>\n\
             Get your API key from https://linkly.ai/dashboard"
        );
    }

    save_credentials_api_key(key)?;
    println!("API key saved to {}", credentials_path()?.display());
    Ok(())
}

/// Handle `linkly auth set-key <token> --lan --endpoint <url>`.
///
/// The token format is Desktop's business, so only emptiness is checked here.
/// Nothing is probed either — same as the remote key; `linkly doctor --lan`
/// is the way to test the saved connection.
pub fn set_lan_key(token: &str, endpoint: &str) -> Result<()> {
    let token = token.trim();
    if token.is_empty() {
        bail!(
            "LAN token is empty.\n\
             Copy it from Linkly AI Desktop > Settings > MCP."
        );
    }
    let endpoint = endpoint.trim();
    if !(endpoint.starts_with("http://") || endpoint.starts_with("https://")) {
        bail!(
            "Invalid LAN endpoint '{}'. Expected a URL like http://192.168.1.100:60606\n\
             Find the address in Linkly AI Desktop > Settings > MCP.",
            endpoint
        );
    }

    save_lan_credentials(endpoint, token)?;
    println!("LAN connection saved to {}", credentials_path()?.display());
    println!(
        "Use --lan to reach {}, e.g. linkly search \"query\" --lan",
        normalize_endpoint(endpoint)
    );
    Ok(())
}

/// Handle `linkly auth status`.
///
/// Reports what is stored locally first, then — only if a remote key is
/// present — asks the gateway whether it is still valid and which plan it
/// resolves to. A network failure degrades to "unknown" rather than failing
/// the command: knowing which key is configured is useful offline. The LAN
/// connection is only reported, never probed (`linkly doctor --lan` does that).
pub async fn status(json_mode: bool) -> Result<()> {
    let stored = read_credentials_api_key().and_then(|key| Ok((key, read_lan_credentials()?)));
    let (key, lan) = match stored {
        Ok(stored) => stored,
        Err(e) => return output::print_error(&e.to_string(), json_mode),
    };
    let lan_json = lan_status_json(lan.as_ref());

    let Some(key) = key else {
        if json_mode {
            println!(
                "{}",
                serde_json::json!({
                    "status": "success",
                    "configured": false,
                    "hint": "Run `linkly auth set-key <api-key>` to configure remote access.",
                    "lan": lan_json,
                })
            );
        } else {
            println!("{}", "Linkly AI Auth".bold());
            println!("  {}  {}", "Key:".dimmed(), "not configured".yellow());
            println!("  {}  {}", "LAN:".dimmed(), lan_summary(lan.as_ref()));
            println!("\n  Run `linkly auth set-key <api-key>` to enable --remote.");
            println!("  Get a key from https://linkly.ai (Dashboard > API Keys).");
            if lan.is_none() {
                println!(
                    "  Run `linkly auth set-key <token> --lan --endpoint <url>` to enable --lan."
                );
            }
        }
        return Ok(());
    };

    let preview = mask_key(&key);
    let path = credentials_path()?;
    let probe = probe_key(&key).await;

    if json_mode {
        let mut obj = serde_json::json!({
            "status": "success",
            "configured": true,
            "key_preview": preview,
            "credentials_path": path.display().to_string(),
            "lan": lan_json,
        });
        match &probe {
            KeyProbe::Valid { plan } => {
                obj["valid"] = serde_json::json!(true);
                obj["plan"] = serde_json::json!(plan);
            }
            KeyProbe::Invalid => obj["valid"] = serde_json::json!(false),
            KeyProbe::Unknown { reason } => {
                obj["valid"] = serde_json::Value::Null;
                obj["probe_error"] = serde_json::json!(reason);
            }
        }
        println!("{}", obj);
        return Ok(());
    }

    println!("{}", "Linkly AI Auth".bold());
    println!("  {}   {}", "Key:".dimmed(), preview);
    match &probe {
        KeyProbe::Valid { plan } => {
            println!("  {} {}", "Valid:".dimmed(), "yes".green());
            println!("  {}  {}", "Plan:".dimmed(), plan);
        }
        KeyProbe::Invalid => {
            println!("  {} {}", "Valid:".dimmed(), "no".red());
        }
        KeyProbe::Unknown { reason } => {
            println!(
                "  {} {} ({})",
                "Valid:".dimmed(),
                "unknown".yellow(),
                reason
            );
        }
    }
    println!("  {}   {}", "LAN:".dimmed(), lan_summary(lan.as_ref()));
    println!("  {}  {}", "File:".dimmed(), path.display());
    // Advice last, so it doesn't split the field list in two.
    if matches!(probe, KeyProbe::Invalid) {
        println!("\n  The stored key was rejected. Run `linkly auth set-key <api-key>` with a current key.");
    }
    Ok(())
}

/// `lan` object of `auth status --json`. The token is never included in full.
fn lan_status_json(lan: Option<&LanCredentials>) -> serde_json::Value {
    match lan {
        Some(lan) => serde_json::json!({
            "configured": true,
            "endpoint": lan.endpoint,
            "token_preview": mask_lan_token(&lan.token),
        }),
        None => serde_json::json!({ "configured": false }),
    }
}

/// One-line human summary of the saved LAN connection.
fn lan_summary(lan: Option<&LanCredentials>) -> String {
    match lan {
        Some(lan) => format!("{} (token {})", lan.endpoint, mask_lan_token(&lan.token)),
        None => "not configured".yellow().to_string(),
    }
}

/// Handle `linkly auth logout [--remote | --lan]`.
///
/// Removes only the targeted credential; the other one stays usable.
/// Idempotent: removing credentials that were never there is a success, not an
/// error — the post-condition the caller asked for already holds.
pub fn logout(target: CredentialTarget, json_mode: bool) -> Result<()> {
    let removed = match target {
        CredentialTarget::Remote => delete_remote_credentials(),
        CredentialTarget::Lan => delete_lan_credentials(),
    };
    let removed = match removed {
        Ok(removed) => removed,
        Err(e) => return output::print_error(&e.to_string(), json_mode),
    };

    if json_mode {
        let target = match target {
            CredentialTarget::Remote => "remote",
            CredentialTarget::Lan => "lan",
        };
        println!(
            "{}",
            serde_json::json!({ "status": "success", "removed": removed, "target": target })
        );
        return Ok(());
    }

    let path = credentials_path()?;
    match (target, removed) {
        (CredentialTarget::Remote, true) => {
            println!("Credentials removed from {}", path.display())
        }
        (CredentialTarget::Remote, false) => {
            println!("No credentials were stored — nothing to remove.")
        }
        (CredentialTarget::Lan, true) => {
            println!("LAN connection removed from {}", path.display())
        }
        (CredentialTarget::Lan, false) => {
            println!("No LAN connection was stored — nothing to remove.")
        }
    }
    Ok(())
}

/// Outcome of asking the gateway about a stored key.
enum KeyProbe {
    Valid { plan: String },
    Invalid,
    Unknown { reason: String },
}

async fn probe_key(key: &str) -> KeyProbe {
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
    {
        Ok(client) => client,
        Err(e) => {
            return KeyProbe::Unknown {
                reason: e.to_string(),
            }
        }
    };

    let resp = match client.get(AUTH_STATUS_URL).bearer_auth(key).send().await {
        Ok(resp) => resp,
        Err(_) => {
            return KeyProbe::Unknown {
                reason: "gateway unreachable".to_string(),
            }
        }
    };

    let code = resp.status().as_u16();
    if code == 401 || code == 403 {
        return KeyProbe::Invalid;
    }
    if !(200..300).contains(&code) {
        return KeyProbe::Unknown {
            reason: format!("gateway returned HTTP {}", code),
        };
    }

    match resp.json::<AuthStatusResponse>().await {
        // A 2xx means the key authenticated. `plan` is additive on the gateway
        // side, so an older deployment that omits it still counts as valid.
        Ok(body) => KeyProbe::Valid {
            plan: display_plan(body.plan.as_deref()),
        },
        Err(_) => KeyProbe::Valid {
            plan: display_plan(None),
        },
    }
}

/// Render the gateway's plan token for humans. Unknown tokens pass through
/// verbatim rather than being coerced to "Free" — mislabelling a paying user's
/// plan is worse than showing a token we don't recognise.
fn display_plan(plan: Option<&str>) -> String {
    match plan {
        Some("pro") => "Pro".to_string(),
        Some("free") => "Free".to_string(),
        Some(other) => other.to_string(),
        None => "unknown".to_string(),
    }
}

/// Mask a key for display: keep the `lkai_` prefix plus four characters at each
/// end, so two keys can be told apart without printing a usable secret.
fn mask_key(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() <= 13 {
        return "*".repeat(chars.len());
    }
    let head: String = chars[..9].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{}…{}", head, tail)
}

/// Mask a LAN token for display: only the last four characters survive, and
/// only when the token is long enough that four characters give little away.
fn mask_lan_token(token: &str) -> String {
    let chars: Vec<char> = token.chars().collect();
    if chars.len() < 16 {
        return "*".repeat(chars.len());
    }
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("…{}", tail)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── T3.2: set-key 验证 ───────────────────────────────

    #[test]
    fn set_key_rejects_short_key() {
        let result = set_key("lkai_short");
        assert!(result.is_err());
    }

    #[test]
    fn set_key_rejects_wrong_prefix() {
        // 37 chars total but wrong prefix
        let result = set_key("wrong_prefix_aaaaaaaabbbbbbbbccccccc");
        assert!(result.is_err());
    }

    #[test]
    fn set_key_rejects_empty() {
        let result = set_key("");
        assert!(result.is_err());
    }

    #[test]
    fn set_key_accepts_valid_format() {
        // lkai_ (5) + 32 hex chars = 37 total
        // set_key writes to HOME dir, so we use shared temp HOME
        crate::test_helpers::with_temp_home("auth_set_key_valid", |_home| {
            let result = set_key("lkai_0123456789abcdef0123456789abcdef");
            assert!(result.is_ok());
        });
    }

    // ── auth status / logout helpers ─────────────────────

    #[test]
    fn mask_key_keeps_prefix_and_tail_only() {
        let masked = mask_key("lkai_d9c64468ee2ebf9e2c16f36e2dc922a7");
        assert_eq!(masked, "lkai_d9c6…22a7");
        // The middle of the secret must not survive masking.
        assert!(!masked.contains("4468ee2ebf9e2c16f36e2dc9"));
    }

    #[test]
    fn mask_key_fully_masks_short_input() {
        // Never reveal a prefix+tail of something too short to spare them.
        assert_eq!(mask_key("lkai_abcd"), "*********");
    }

    #[test]
    fn display_plan_maps_known_tokens_and_passes_through_others() {
        assert_eq!(display_plan(Some("pro")), "Pro");
        assert_eq!(display_plan(Some("free")), "Free");
        assert_eq!(display_plan(Some("enterprise")), "enterprise");
        assert_eq!(display_plan(None), "unknown");
    }

    #[test]
    fn logout_is_idempotent_when_nothing_is_stored() {
        crate::test_helpers::with_temp_home("auth_logout_empty", |_home| {
            // No credentials written — logout must still succeed.
            assert!(logout(CredentialTarget::Remote, false).is_ok());
            assert!(logout(CredentialTarget::Lan, false).is_ok());
        });
    }

    #[test]
    fn logout_removes_a_stored_key() {
        crate::test_helpers::with_temp_home("auth_logout_removes", |home| {
            set_key("lkai_0123456789abcdef0123456789abcdef").expect("set_key");
            assert!(read_credentials_api_key().expect("read").is_some());
            assert!(logout(CredentialTarget::Remote, false).is_ok());
            assert!(read_credentials_api_key()
                .expect("read after logout")
                .is_none());
            // With nothing else stored, the file goes away as it always did.
            assert!(!home.join(".linkly").join("credentials.json").exists());
        });
    }

    // ── LAN credentials ──────────────────────────────────

    const REMOTE_KEY: &str = "lkai_0123456789abcdef0123456789abcdef";

    #[test]
    fn set_lan_key_rejects_empty_token() {
        crate::test_helpers::with_temp_home("auth_lan_empty_token", |_home| {
            assert!(set_lan_key("  ", "http://192.168.1.5:60606").is_err());
            assert!(read_lan_credentials().expect("read").is_none());
        });
    }

    #[test]
    fn set_lan_key_rejects_endpoint_without_scheme() {
        crate::test_helpers::with_temp_home("auth_lan_bad_endpoint", |_home| {
            assert!(set_lan_key("5a56594a664d6350", "192.168.1.5:60606").is_err());
            assert!(read_lan_credentials().expect("read").is_none());
        });
    }

    #[test]
    fn set_lan_key_accepts_any_non_empty_token_and_normalizes_endpoint() {
        crate::test_helpers::with_temp_home("auth_lan_saved", |_home| {
            // Not lkai_-shaped: the LAN token format belongs to Desktop.
            set_lan_key(" tok ", "http://192.168.1.5:60606/mcp/").expect("set_lan_key");
            assert_eq!(
                read_lan_credentials().expect("read"),
                Some(LanCredentials {
                    endpoint: "http://192.168.1.5:60606".to_string(),
                    token: "tok".to_string(),
                })
            );
        });
    }

    #[test]
    fn remote_and_lan_keys_do_not_overwrite_each_other() {
        crate::test_helpers::with_temp_home("auth_both_kept", |_home| {
            set_lan_key("lan-token-1234567", "http://192.168.1.5:60606").expect("lan");
            set_key(REMOTE_KEY).expect("remote");
            set_lan_key("lan-token-7654321", "http://192.168.1.6:60606").expect("lan again");

            assert_eq!(
                read_credentials_api_key().unwrap().as_deref(),
                Some(REMOTE_KEY)
            );
            let lan = read_lan_credentials().unwrap().expect("lan kept");
            assert_eq!(lan.endpoint, "http://192.168.1.6:60606");
            assert_eq!(lan.token, "lan-token-7654321");
        });
    }

    #[test]
    fn logout_removes_only_the_targeted_credential() {
        crate::test_helpers::with_temp_home("auth_logout_targeted", |home| {
            set_key(REMOTE_KEY).expect("remote");
            set_lan_key("lan-token-1234567", "http://192.168.1.5:60606").expect("lan");

            logout(CredentialTarget::Lan, false).expect("logout --lan");
            assert!(read_lan_credentials().unwrap().is_none());
            assert_eq!(
                read_credentials_api_key().unwrap().as_deref(),
                Some(REMOTE_KEY)
            );

            set_lan_key("lan-token-1234567", "http://192.168.1.5:60606").expect("lan");
            logout(CredentialTarget::Remote, false).expect("logout");
            assert!(read_credentials_api_key().unwrap().is_none());
            assert!(read_lan_credentials().unwrap().is_some());

            logout(CredentialTarget::Lan, false).expect("logout --lan");
            assert!(!home.join(".linkly").join("credentials.json").exists());
        });
    }

    #[test]
    fn mask_lan_token_keeps_only_the_tail() {
        assert_eq!(mask_lan_token("5a56594a664d6350"), "…6350");
        assert_eq!(mask_lan_token("short"), "*****");
    }
}
