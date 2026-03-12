use anyhow::{Result, bail};

use crate::connection::save_credentials_api_key;

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
    println!("API key saved to ~/.linkly/credentials.json");
    Ok(())
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
}
