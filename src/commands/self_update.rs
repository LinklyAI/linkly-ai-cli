use anyhow::{Context, Result, bail};
use owo_colors::OwoColorize;
use serde::Deserialize;
use std::io::Read;
use std::path::Path;

const LATEST_URL: &str = "https://updater.linkly.ai/cli/latest.json";

#[derive(Deserialize)]
struct LatestInfo {
    version: String,
    assets: std::collections::HashMap<String, String>,
}

/// Run the self-update command interactively.
pub async fn run() -> Result<()> {
    let current = semver::Version::parse(env!("CARGO_PKG_VERSION"))?;
    println!(
        "Current version: v{}",
        env!("CARGO_PKG_VERSION").dimmed()
    );
    println!("Checking for updates...");

    let info = fetch_latest().await?;
    let latest = semver::Version::parse(&info.version)
        .with_context(|| format!("Invalid version in latest.json: {}", info.version))?;

    if latest <= current {
        println!("{}", "Already up to date!".green());
        return Ok(());
    }

    println!(
        "New version available: v{} → v{}",
        current.dimmed(),
        latest.green()
    );

    let platform_key = platform_key()?;
    let download_url = info
        .assets
        .get(&platform_key)
        .with_context(|| format!("No binary available for platform: {}", platform_key))?;

    println!("Downloading {}...", platform_key);

    let bytes = reqwest::get(download_url)
        .await?
        .error_for_status()?
        .bytes()
        .await?;

    println!("Installing...");

    let tmp_dir = tempfile::tempdir()?;
    let binary_path = extract_binary(&bytes, &tmp_dir, &platform_key)?;
    self_replace::self_replace(&binary_path)?;

    println!(
        "{} Updated to v{}",
        "Success!".green().bold(),
        latest
    );

    Ok(())
}

/// Check for new version silently (for startup hint).
/// Returns Some(version_string) if a newer version is available.
pub async fn check_silently() -> Option<String> {
    let current = semver::Version::parse(env!("CARGO_PKG_VERSION")).ok()?;
    let info = fetch_latest().await.ok()?;
    let latest = semver::Version::parse(&info.version).ok()?;
    if latest > current {
        Some(info.version)
    } else {
        None
    }
}

async fn fetch_latest() -> Result<LatestInfo> {
    let resp = reqwest::get(LATEST_URL)
        .await
        .context("Failed to check for updates")?
        .error_for_status()
        .context("Update server returned an error")?;
    resp.json::<LatestInfo>()
        .await
        .context("Invalid response from update server")
}

fn platform_key() -> Result<String> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let target = match (os, arch) {
        ("macos", "aarch64") => "darwin-aarch64",
        ("macos", "x86_64") => "darwin-x86_64",
        ("linux", "x86_64") => "linux-x86_64",
        ("linux", "aarch64") => "linux-aarch64",
        ("windows", "x86_64") => "windows-x86_64",
        _ => bail!("Unsupported platform: {}-{}", os, arch),
    };
    Ok(target.to_string())
}

/// Extract the `linkly` binary from a .tar.gz or .zip archive.
fn extract_binary(
    bytes: &[u8],
    tmp_dir: &tempfile::TempDir,
    platform_key: &str,
) -> Result<std::path::PathBuf> {
    if platform_key.starts_with("windows") {
        extract_from_zip(bytes, tmp_dir)
    } else {
        extract_from_tar_gz(bytes, tmp_dir)
    }
}

fn extract_from_tar_gz(bytes: &[u8], tmp_dir: &tempfile::TempDir) -> Result<std::path::PathBuf> {
    let decoder = flate2::read::GzDecoder::new(bytes);
    let mut archive = tar::Archive::new(decoder);

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();

        if file_name == "linkly" {
            let dest = tmp_dir.path().join("linkly");
            entry.unpack(&dest)?;
            return Ok(dest);
        }
    }

    bail!("Binary 'linkly' not found in archive")
}

fn extract_from_zip(bytes: &[u8], tmp_dir: &tempfile::TempDir) -> Result<std::path::PathBuf> {
    let cursor = std::io::Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(cursor)?;

    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        let name = file.name().to_string();
        let file_name = Path::new(&name)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();

        if file_name == "linkly.exe" || file_name == "linkly" {
            let dest = tmp_dir.path().join(file_name);
            let mut out = std::fs::File::create(&dest)?;
            let mut buf = Vec::new();
            file.read_to_end(&mut buf)?;
            std::io::Write::write_all(&mut out, &buf)?;
            return Ok(dest);
        }
    }

    bail!("Binary 'linkly' not found in zip archive")
}
