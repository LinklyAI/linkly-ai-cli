/// Supported document types for search filtering.
///
/// Mirrors `doc_type_from_ext` in the desktop app's scanner — the desktop is the
/// authoritative source and performs **no** validation of its own (`doc_types`
/// goes straight into the search index as a term). This list is therefore the
/// only gate a `--type` value passes through, which cuts both ways: it catches
/// typos before a pointless round-trip, but it also rejects legitimate types
/// whenever the desktop gains a format and this list isn't updated with it.
///
/// **When the desktop adds a document type, add it here in the same change.**
/// Derived types (`image` = OCR text, `audio` / `video` = transcripts) count.
///
/// Note: the MCP bridge (`linkly mcp`) passes `doc_types` through without
/// validation, so an out-of-date CLI never blocks a bridged client.
pub const VALID_DOC_TYPES: &[&str] = &[
    "pdf", "md", "xlsx", "csv", "docx", "txt", "pptx", "doc", "rtf", "epub", "html", "image",
    "audio", "video",
];

/// How an agent signs in, appended to every hint that suggests
/// `linkly auth login`: the plain command waits in the foreground until the
/// link is approved, so an agent's shell tool would never show the link to
/// the user. A macro so it can be spliced into `concat!` literals.
#[macro_export]
macro_rules! agent_login_hint {
    () => {
        "(agents: `linkly auth login --no-wait --json`, then `linkly auth login --wait --json`)"
    };
}

/// The one way every command explains a rejected remote API key.
#[macro_export]
macro_rules! key_rejected_hint {
    () => {
        concat!(
            "Your API key was rejected. Run `linkly auth logout`, then `linkly auth login` ",
            $crate::agent_login_hint!(),
            ", or save a new key with `linkly auth set-key <api-key>`."
        )
    };
}

/// Linkly AI website, which hosts the browser side of `linkly auth login`
/// (the approval page and the login API).
pub const WEB_BASE_URL: &str = "https://linkly.ai";

/// Overrides [`WEB_BASE_URL`], e.g. `http://localhost:3000` to test login
/// against a local web server. Not needed in normal use.
pub const WEB_BASE_URL_ENV: &str = "LINKLY_WEB_URL";

/// The website base URL in effect: `$LINKLY_WEB_URL` (trailing `/` dropped) or
/// [`WEB_BASE_URL`].
///
/// The login hands an API key over this connection, so an override must be
/// https — plain http is accepted only for this machine (local development).
pub fn web_base_url() -> anyhow::Result<String> {
    match std::env::var(WEB_BASE_URL_ENV) {
        Ok(url) if !url.trim().is_empty() => {
            let url = url.trim().trim_end_matches('/').to_string();
            if is_acceptable_web_base(&url) {
                Ok(url)
            } else {
                anyhow::bail!(
                    "{} must be an https:// URL (http:// only for localhost), got '{}'. \
                     Unset it to use {}.",
                    WEB_BASE_URL_ENV,
                    url,
                    WEB_BASE_URL
                )
            }
        }
        _ => Ok(WEB_BASE_URL.to_string()),
    }
}

/// https to anywhere, or http to a loopback host.
pub fn is_acceptable_web_base(url: &str) -> bool {
    if let Some(rest) = url.strip_prefix("https://") {
        return !rest.is_empty();
    }
    let Some(rest) = url.strip_prefix("http://") else {
        return false;
    };
    let authority = rest.split('/').next().unwrap_or_default();
    let host = match authority.rsplit_once(':') {
        Some((h, port)) if port.chars().all(|c| c.is_ascii_digit()) => h,
        _ => authority,
    };
    let host = host.trim_matches(|c| c == '[' || c == ']');
    host == "localhost" || host == "::1" || host.starts_with("127.")
}

#[cfg(test)]
mod web_base_tests {
    use super::is_acceptable_web_base;

    #[test]
    fn only_https_or_loopback_http_is_accepted() {
        for ok in [
            "https://linkly.ai",
            "https://preview.example.com",
            "http://localhost:3000",
            "http://127.0.0.1:3000",
            "http://[::1]:3000",
        ] {
            assert!(is_acceptable_web_base(ok), "{ok}");
        }
        for bad in [
            "http://192.168.1.5:3000",
            "http://linkly.ai",
            "http://localhost.evil.com",
            "ftp://linkly.ai",
            "linkly.ai",
            "https://",
        ] {
            assert!(!is_acceptable_web_base(bad), "{bad}");
        }
    }
}
