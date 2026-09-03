//! Detection, version reading and the update notice for the `linkly-ai` skill.
//!
//! The notice reaches agents through tool results rather than stderr. The CLI's
//! own update hint has always gone to stderr after `run()` returns, which under
//! `linkly mcp` means it is emitted once at process shutdown into a stream MCP
//! clients discard — so no agent has ever seen it. Anything meant for an agent
//! has to travel on the same channel as the answer.
//!
//! SYNC: the install locations mirror `src-tauri/src/integrator/paths.rs` in
//! linkly-ai-desktop-v3, which owns that table. When Desktop learns a new
//! client, add its directory here too; otherwise the check reports "not
//! installed" on a machine that has the skill.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Directory name the skill occupies inside every skills root.
pub const SKILL_DIR_NAME: &str = "linkly-ai";

const LATEST_URL: &str = "https://updater.linkly.ai/skills/latest.json";
/// Last-resort download location, used only when latest.json cannot be read —
/// at which point the version is unknown anyway, so there is nothing better to
/// aim at. Prefer [`Latest::url`]: this rolling path sits behind a multi-hour
/// CDN cache and serves the previous package for a while after each release.
const FALLBACK_ZIP_URL: &str = "https://updater.linkly.ai/skills/linkly-skills-latest.zip";
const DOCS_URL: &str = "https://linkly.ai/docs/en/use-skills";

const STATE_FILE: &str = "skills-check.json";
const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
/// Same bound as the CLI's own update check: an unreachable updater host must
/// not hold up the command the user actually ran.
const FETCH_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Deserialize)]
struct LatestInfo {
    version: String,
}

/// What the update server currently publishes.
pub struct Latest {
    pub version: semver::Version,
    pub url: String,
}

#[derive(Serialize, Deserialize)]
struct CheckState {
    /// UTC ISO 8601 timestamp of the last completed check.
    last_checked_at: String,
}

/// What a local install looks like. The four cases lead to different
/// conclusions, and collapsing any two of them produces a wrong notice.
#[derive(Debug, PartialEq, Eq)]
pub enum Local {
    /// No skill directory in any known location.
    Missing,
    /// Installed, but neither version marker is present. That can only mean the
    /// copy predates version tracking, so it is necessarily out of date.
    Untracked(PathBuf),
    /// A version is recorded but is not valid semver — edited by hand, or a
    /// fork. Reporting it as outdated would be a false alarm.
    Unparseable(PathBuf),
    Tracked(PathBuf, semver::Version),
}

/// The single real store; every other location is a link to it.
pub fn source_dir() -> Option<PathBuf> {
    Some(
        dirs::home_dir()?
            .join(".agents")
            .join("skills")
            .join(SKILL_DIR_NAME),
    )
}

/// Locations to inspect and to update, most authoritative first.
pub fn known_locations() -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    vec![
        home.join(".agents").join("skills").join(SKILL_DIR_NAME),
        home.join(".claude").join("skills").join(SKILL_DIR_NAME),
    ]
}

/// Read the version out of a `SKILL.md`.
///
/// The body marker wins over the frontmatter key. Some platforms strip
/// frontmatter fields they do not recognise, and a rewritten frontmatter is
/// exactly the case where the two disagree; body text always travels with the
/// file. Returns `None` when neither marker is present, `Some(Err)` shape via
/// the caller when one exists but does not parse.
fn read_version_string(skill_md: &Path) -> Option<String> {
    let text = std::fs::read_to_string(skill_md).ok()?;

    let body = text.lines().find_map(|line| {
        line.strip_prefix("linkly-ai-skill-version:")
            .map(|rest| rest.trim().to_string())
    });
    if body.is_some() {
        return body.filter(|v| !v.is_empty());
    }

    // Frontmatter: the block between the first two `---` lines.
    let mut in_front = false;
    for line in text.lines() {
        if line.trim() == "---" {
            if in_front {
                break;
            }
            in_front = true;
            continue;
        }
        if in_front {
            if let Some(rest) = line.strip_prefix("version:") {
                let v = rest.trim().to_string();
                return (!v.is_empty()).then_some(v);
            }
        }
    }
    None
}

/// Inspect the known locations and classify the install.
pub fn detect() -> Local {
    for dir in known_locations() {
        let skill_md = dir.join("SKILL.md");
        if !skill_md.exists() {
            continue;
        }
        return match read_version_string(&skill_md) {
            None => Local::Untracked(dir),
            Some(raw) => match semver::Version::parse(&raw) {
                Ok(v) => Local::Tracked(dir, v),
                Err(_) => Local::Unparseable(dir),
            },
        };
    }
    Local::Missing
}

pub async fn fetch_latest() -> Result<Latest> {
    let client = reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .build()
        .context("Failed to build HTTP client")?;
    let info: LatestInfo = client
        .get(LATEST_URL)
        .send()
        .await
        .context("Failed to reach the skills update server")?
        .error_for_status()
        .context("Skills update server returned an error")?
        .json()
        .await
        .context("Invalid response from the skills update server")?;
    let version = semver::Version::parse(&info.version)
        .with_context(|| format!("Invalid version in skills latest.json: {}", info.version))?;
    // The download path is derived here rather than taken from latest.json.
    // A server-supplied URL is only as good as whatever was published last —
    // and a latest.json still pointing at the rolling file would hand us the
    // CDN's cached copy of the previous release, which is the exact failure
    // this is meant to avoid. The versioned path is immutable, so deriving it
    // from a version we just read cannot be stale.
    // SYNC: the layout is written by .github/workflows/release.yml in
    // linkly-ai-skills; changing it there requires changing it here.
    let url = format!("https://updater.linkly.ai/skills/v{version}/linkly-skills.zip");
    Ok(Latest { version, url })
}

/// Where to download from when the update server is unreachable.
pub fn fallback_zip_url() -> &'static str {
    FALLBACK_ZIP_URL
}

fn state_path() -> Option<PathBuf> {
    Some(dirs::home_dir()?.join(".linkly").join(STATE_FILE))
}

/// Whether enough time has passed since the last completed check.
///
/// A missing or unreadable state file means "due": the cost of one extra check
/// is a 3-second bounded request, while treating a corrupt file as "not due"
/// would silence the notice permanently.
fn due_for_check() -> bool {
    let Some(path) = state_path() else {
        return false;
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return true;
    };
    let Ok(state) = serde_json::from_str::<CheckState>(&text) else {
        return true;
    };
    let Ok(last) = chrono::DateTime::parse_from_rfc3339(&state.last_checked_at) else {
        return true;
    };
    let elapsed = chrono::Utc::now().signed_duration_since(last.with_timezone(&chrono::Utc));
    elapsed
        .to_std()
        .map(|e| e >= CHECK_INTERVAL)
        .unwrap_or(true)
}

/// Best-effort; a failure here only means the next run checks again.
fn record_checked() {
    let Some(path) = state_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let state = CheckState {
        last_checked_at: chrono::Utc::now().to_rfc3339(),
    };
    if let Ok(text) = serde_json::to_string(&state) {
        let _ = std::fs::write(&path, text);
    }
}

/// The notice, once computed, for output paths that cannot await it.
///
/// `main` runs the check concurrently with the command itself; plain-text
/// output awaits the task and is therefore always accurate. JSON output is
/// printed from inside the command, before the task is joined, so it reads
/// this slot and simply omits the field when the check has not landed yet —
/// a machine-readable envelope is better off missing an advisory field than
/// waiting on the network for it.
static HINT: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();

pub fn publish_hint(hint: Option<String>) {
    let _ = HINT.set(hint);
}

pub fn hint() -> Option<&'static str> {
    HINT.get().and_then(|h| h.as_deref())
}

/// One line for the current state, or `None` when there is nothing worth
/// saying. Never returns an error: a failed check must be indistinguishable
/// from "everything is fine", or offline users get a permanent complaint.
pub async fn check_silently() -> Option<String> {
    if !due_for_check() {
        return None;
    }

    match detect() {
        // A hand-edited or forked copy. Silence beats a false alarm.
        Local::Unparseable(_) => {
            record_checked();
            None
        }
        // Purely local, so this one still works offline.
        Local::Missing => {
            record_checked();
            Some(format!(
                "[linkly] Skills not installed. Install: `linkly skills install` — docs: {DOCS_URL}"
            ))
        }
        Local::Untracked(_) => {
            let latest = fetch_latest().await.ok()?.version;
            record_checked();
            Some(format!(
                "[linkly] Skills: installed copy predates version tracking, v{latest} available. \
                 Update: `linkly skills update`"
            ))
        }
        Local::Tracked(_, current) => {
            let latest = fetch_latest().await.ok()?.version;
            record_checked();
            (latest > current).then(|| {
                format!(
                    "[linkly] Skills v{current} installed, v{latest} available. \
                     Update: `linkly skills update`"
                )
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn skill_md(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("SKILL.md");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(body.as_bytes()).unwrap();
        path
    }

    #[test]
    fn body_marker_is_read() {
        let tmp = tempfile::tempdir().unwrap();
        let p = skill_md(tmp.path(), "# Skill\n\nlinkly-ai-skill-version: 1.2.3\n");
        assert_eq!(read_version_string(&p).as_deref(), Some("1.2.3"));
    }

    #[test]
    fn frontmatter_is_the_fallback() {
        let tmp = tempfile::tempdir().unwrap();
        let p = skill_md(tmp.path(), "---\nname: x\nversion: 4.5.6\n---\n\n# Skill\n");
        assert_eq!(read_version_string(&p).as_deref(), Some("4.5.6"));
    }

    /// A platform that rewrites frontmatter is exactly when the two disagree,
    /// and the body is the copy that survived untouched.
    #[test]
    fn body_marker_wins_over_frontmatter() {
        let tmp = tempfile::tempdir().unwrap();
        let p = skill_md(
            tmp.path(),
            "---\nversion: 0.0.1\n---\n\n# Skill\n\nlinkly-ai-skill-version: 9.9.9\n",
        );
        assert_eq!(read_version_string(&p).as_deref(), Some("9.9.9"));
    }

    #[test]
    fn a_skill_without_any_marker_reads_as_none() {
        let tmp = tempfile::tempdir().unwrap();
        let p = skill_md(tmp.path(), "---\nname: x\n---\n\n# Skill\n");
        assert_eq!(read_version_string(&p), None);
    }

    /// `version:` outside the frontmatter block must not be picked up — the
    /// skill body discusses versions in prose.
    #[test]
    fn version_in_prose_is_not_a_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let p = skill_md(tmp.path(), "---\nname: x\n---\n\nversion: not-a-marker\n");
        assert_eq!(read_version_string(&p), None);
    }
}
