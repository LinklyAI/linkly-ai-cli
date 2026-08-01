use anyhow::{bail, Result};
use owo_colors::OwoColorize;
use serde::Deserialize;

use crate::connection::{
    credentials_path, delete_credentials, read_credentials_api_key, save_credentials_api_key,
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

/// Handle `linkly auth set-key <key>`
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

/// Handle `linkly auth status`.
///
/// Reports what is stored locally first, then — only if a key is present —
/// asks the gateway whether it is still valid and which plan it resolves to.
/// A network failure degrades to "unknown" rather than failing the command:
/// knowing which key is configured is useful offline.
pub async fn status(json_mode: bool) -> Result<()> {
    let key = match read_credentials_api_key() {
        Ok(key) => key,
        Err(e) => return output::print_error(&e.to_string(), json_mode),
    };

    let Some(key) = key else {
        if json_mode {
            println!(
                "{}",
                serde_json::json!({
                    "status": "success",
                    "configured": false,
                    "hint": "Run `linkly auth set-key <api-key>` to configure remote access.",
                })
            );
        } else {
            println!("{}", "Linkly AI Auth".bold());
            println!("  {}  {}", "Key:".dimmed(), "not configured".yellow());
            println!("\n  Run `linkly auth set-key <api-key>` to enable --remote.");
            println!("  Get a key from https://linkly.ai (Dashboard > API Keys).");
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
            println!("\n  The stored key was rejected. Run `linkly auth set-key <api-key>` with a current key.");
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
    println!("  {}  {}", "File:".dimmed(), path.display());
    Ok(())
}

/// Handle `linkly auth logout`.
///
/// Idempotent: removing credentials that were never there is a success, not an
/// error — the post-condition the caller asked for already holds.
pub fn logout(json_mode: bool) -> Result<()> {
    let removed = match delete_credentials() {
        Ok(removed) => removed,
        Err(e) => return output::print_error(&e.to_string(), json_mode),
    };

    if json_mode {
        println!(
            "{}",
            serde_json::json!({ "status": "success", "removed": removed })
        );
    } else if removed {
        println!("Credentials removed from {}", credentials_path()?.display());
    } else {
        println!("No credentials were stored — nothing to remove.");
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
            assert!(logout(false).is_ok());
        });
    }

    #[test]
    fn logout_removes_a_stored_key() {
        crate::test_helpers::with_temp_home("auth_logout_removes", |_home| {
            set_key("lkai_0123456789abcdef0123456789abcdef").expect("set_key");
            assert!(read_credentials_api_key().expect("read").is_some());
            assert!(logout(false).is_ok());
            assert!(read_credentials_api_key()
                .expect("read after logout")
                .is_none());
        });
    }
}
