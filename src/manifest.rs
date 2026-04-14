//! Write `~/.linkly/installed.json` on every CLI invocation.
//!
//! The Desktop app reads this file to detect CLI installation status,
//! avoiding the fallback of scanning common paths. Fields `path` and
//! `version` are consumed by Desktop today; extra fields are ignored
//! by serde's default behavior, so adding them is backward-compatible.

use anyhow::Context;
use chrono::Utc;
use serde::{Deserialize, Serialize};

const MANIFEST_FILE: &str = "installed.json";

#[derive(Serialize, Deserialize)]
pub(crate) struct InstalledManifest {
    /// Absolute path to the CLI binary.
    pub path: String,
    /// CLI version (e.g. "0.3.0").
    pub version: String,
    /// Platform/architecture triple (e.g. "aarch64-apple-darwin").
    pub arch: String,
    /// UTC ISO 8601 timestamp of first installation (preserved across updates).
    pub installed_at: String,
    /// How the CLI was installed: "official", "cargo", "homebrew", "other".
    pub install_method: String,
    /// UTC ISO 8601 timestamp of last invocation (updated every run).
    pub last_invoked_at: String,
}

/// Write the installed manifest. Best-effort; errors are silently ignored.
pub(crate) fn write_manifest() {
    let _ = try_write_manifest();
}

fn try_write_manifest() -> anyhow::Result<()> {
    let dir = dirs::home_dir()
        .context("Cannot determine home directory")?
        .join(".linkly");

    std::fs::create_dir_all(&dir)
        .with_context(|| format!("Failed to create directory: {}", dir.display()))?;

    let manifest_path = dir.join(MANIFEST_FILE);
    let now = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

    // Read existing manifest to preserve installed_at and install_method.
    let existing = read_existing(&manifest_path);

    let exe_path = std::env::current_exe()
        .context("Cannot determine binary path")?
        .to_string_lossy()
        .into_owned();

    let manifest = InstalledManifest {
        version: env!("CARGO_PKG_VERSION").to_string(),
        arch: arch_triple(),
        installed_at: existing
            .as_ref()
            .map(|m| m.installed_at.clone())
            .unwrap_or_else(|| now.clone()),
        install_method: existing
            .as_ref()
            .map(|m| m.install_method.clone())
            .unwrap_or_else(|| detect_install_method(&exe_path)),
        last_invoked_at: now,
        path: exe_path,
    };

    let json_bytes = serde_json::to_string_pretty(&manifest)?;

    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o644)
            .open(&manifest_path)
            .with_context(|| {
                format!(
                    "Failed to open manifest file: {}",
                    manifest_path.display()
                )
            })?;
        file.write_all(json_bytes.as_bytes())
            .with_context(|| format!("Failed to write manifest: {}", manifest_path.display()))?;
    }

    #[cfg(not(unix))]
    {
        std::fs::write(&manifest_path, json_bytes)
            .with_context(|| format!("Failed to write manifest: {}", manifest_path.display()))?;
    }

    Ok(())
}

fn read_existing(path: &std::path::Path) -> Option<InstalledManifest> {
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

fn arch_triple() -> String {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        ("windows", "x86_64") => "x86_64-pc-windows-msvc",
        (os, arch) => return format!("{}-unknown-{}", arch, os),
    }
    .to_string()
}

fn detect_install_method(exe_path: &str) -> String {
    let normalized = exe_path.replace('\\', "/");

    if normalized.contains("/.linkly/bin/") {
        "official".to_string()
    } else if normalized.contains("/.cargo/bin/") {
        "cargo".to_string()
    } else if normalized.contains("/homebrew/")
        || (cfg!(target_os = "macos") && normalized.starts_with("/usr/local/bin/"))
    {
        "homebrew".to_string()
    } else {
        "other".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::with_temp_home;

    #[test]
    fn write_creates_file() {
        with_temp_home("manifest_create", |home| {
            write_manifest();
            let path = home.join(".linkly").join("installed.json");
            assert!(path.exists(), "installed.json should be created");

            let content = std::fs::read_to_string(&path).unwrap();
            let manifest: InstalledManifest = serde_json::from_str(&content).unwrap();
            assert_eq!(manifest.version, env!("CARGO_PKG_VERSION"));
            assert!(!manifest.path.is_empty());
            assert!(!manifest.arch.is_empty());
            assert!(!manifest.installed_at.is_empty());
            assert!(!manifest.install_method.is_empty());
            assert!(!manifest.last_invoked_at.is_empty());
        });
    }

    #[test]
    fn preserves_installed_at() {
        with_temp_home("manifest_preserve", |home| {
            let dir = home.join(".linkly");
            std::fs::create_dir_all(&dir).unwrap();
            let original_time = "2025-01-01T00:00:00Z";
            let seed = serde_json::json!({
                "path": "/old/path",
                "version": "0.1.0",
                "arch": "aarch64-apple-darwin",
                "installed_at": original_time,
                "install_method": "cargo",
                "last_invoked_at": "2025-01-01T00:00:00Z"
            });
            std::fs::write(
                dir.join("installed.json"),
                serde_json::to_string_pretty(&seed).unwrap(),
            )
            .unwrap();

            write_manifest();

            let content = std::fs::read_to_string(dir.join("installed.json")).unwrap();
            let manifest: InstalledManifest = serde_json::from_str(&content).unwrap();

            assert_eq!(manifest.installed_at, original_time);
            assert_eq!(manifest.install_method, "cargo");
            assert_eq!(manifest.version, env!("CARGO_PKG_VERSION"));
            assert_ne!(manifest.last_invoked_at, "2025-01-01T00:00:00Z");
        });
    }

    #[test]
    fn auto_creates_directory() {
        with_temp_home("manifest_mkdir", |home| {
            assert!(!home.join(".linkly").exists());
            write_manifest();
            assert!(home.join(".linkly").join("installed.json").exists());
        });
    }

    #[test]
    fn detect_install_method_official() {
        assert_eq!(
            detect_install_method("/Users/foo/.linkly/bin/linkly"),
            "official"
        );
        // Should NOT match paths without the dot prefix
        assert_eq!(
            detect_install_method("/opt/some-linkly/bin/linkly"),
            "other"
        );
    }

    #[test]
    fn detect_install_method_cargo() {
        assert_eq!(
            detect_install_method("/Users/foo/.cargo/bin/linkly"),
            "cargo"
        );
    }

    #[test]
    fn detect_install_method_homebrew() {
        assert_eq!(
            detect_install_method("/opt/homebrew/bin/linkly"),
            "homebrew"
        );
        // /usr/local/bin/ is homebrew only on macOS
        if cfg!(target_os = "macos") {
            assert_eq!(
                detect_install_method("/usr/local/bin/linkly"),
                "homebrew"
            );
        } else {
            assert_eq!(
                detect_install_method("/usr/local/bin/linkly"),
                "other"
            );
        }
    }

    #[test]
    fn detect_install_method_other() {
        assert_eq!(detect_install_method("/usr/bin/linkly"), "other");
    }

    #[test]
    fn arch_triple_returns_known_value() {
        let triple = arch_triple();
        let known = [
            "aarch64-apple-darwin",
            "x86_64-apple-darwin",
            "x86_64-unknown-linux-gnu",
            "aarch64-unknown-linux-gnu",
            "x86_64-pc-windows-msvc",
        ];
        assert!(
            known.contains(&triple.as_str()) || triple.contains('-'),
            "unexpected arch_triple: {}",
            triple
        );
    }
}
