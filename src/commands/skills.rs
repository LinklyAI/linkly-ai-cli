//! `linkly skills status | install | update`.
//!
//! Installing is deliberately a command, not something the version check does
//! on its own. The skill can be installed four different ways, each with its
//! own upgrade action, and `~/.agents/skills/` is a shared directory holding
//! other tools' skills — so the CLI touches exactly one subdirectory, and only
//! when a person or an agent asks it to.

use std::io::Read;
use std::path::Path;

use anyhow::{bail, Context, Result};
use owo_colors::OwoColorize;

use crate::skills::{self, Local};

/// What a location on disk turned out to be, which decides how to update it.
enum Form {
    /// Follows the real store; updating the store is enough.
    Link,
    /// A working copy under version control. Overwriting it would discard the
    /// user's checkout, so it gets an instruction instead.
    GitCheckout,
    /// A real directory holding its own copy — Desktop falls back to copying
    /// when it cannot create a symlink, so this is not hypothetical. Left
    /// alone, it stays on the old version forever.
    Directory,
    Absent,
}

fn classify(path: &Path) -> Form {
    match std::fs::symlink_metadata(path) {
        Err(_) => Form::Absent,
        Ok(meta) => {
            if meta.file_type().is_symlink() {
                Form::Link
            } else if path.join(".git").exists() {
                Form::GitCheckout
            } else {
                Form::Directory
            }
        }
    }
}

pub async fn status(json_mode: bool) -> Result<()> {
    let local = skills::detect();
    let latest = skills::fetch_latest().await.ok();

    let installed = match &local {
        Local::Missing => None,
        Local::Untracked(_) => Some("unknown (predates version tracking)".to_string()),
        Local::Unparseable(_) => Some("unrecognised".to_string()),
        Local::Tracked(_, v) => Some(v.to_string()),
    };

    if json_mode {
        let locations: Vec<_> = skills::known_locations()
            .into_iter()
            .map(|p| {
                serde_json::json!({
                    "path": p.display().to_string(),
                    "form": match classify(&p) {
                        Form::Link => "link",
                        Form::GitCheckout => "git",
                        Form::Directory => "directory",
                        Form::Absent => "absent",
                    },
                })
            })
            .collect();
        let payload = serde_json::json!({
            "installed": installed,
            "latest": latest.as_ref().map(|v| v.to_string()),
            "locations": locations,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    match installed {
        None => println!("Skills: {}", "not installed".yellow()),
        Some(v) => println!("Skills installed: {}", v),
    }
    match &latest {
        Some(v) => println!("Latest published: {}", v),
        None => println!("Latest published: {}", "unavailable".dimmed()),
    }
    println!();
    for path in skills::known_locations() {
        let form = match classify(&path) {
            Form::Link => "link",
            Form::GitCheckout => "git checkout",
            Form::Directory => "directory",
            Form::Absent => "-",
        };
        println!("  {:<12} {}", form, path.display());
    }
    Ok(())
}

pub async fn install() -> Result<()> {
    let source = skills::source_dir().context("Cannot determine home directory")?;
    let bytes = download().await?;

    println!("Installing to {}", source.display());
    replace_dir(&source, &bytes)?;

    for path in skills::known_locations() {
        if path == source {
            continue;
        }
        match classify(&path) {
            Form::Absent => link_or_copy(&source, &path)?,
            Form::Link => {}
            Form::GitCheckout | Form::Directory => {
                replace_dir(&path, &bytes)?;
            }
        }
    }

    println!("{}", "Skills installed.".green());
    Ok(())
}

pub async fn update() -> Result<()> {
    if skills::detect() == Local::Missing {
        println!("Skills are not installed yet; installing.");
        return install().await;
    }

    let bytes = download().await?;
    let mut touched = 0usize;

    for path in skills::known_locations() {
        match classify(&path) {
            Form::Absent => {}
            // The real store carries the content; a link needs nothing.
            Form::Link => println!("  {} {}", "link".dimmed(), path.display()),
            Form::GitCheckout => {
                println!(
                    "  {} {} — run `git pull` there instead; leaving it untouched",
                    "git".yellow(),
                    path.display()
                );
            }
            Form::Directory => {
                replace_dir(&path, &bytes)?;
                println!("  {} {}", "updated".green(), path.display());
                touched += 1;
            }
        }
    }

    if touched == 0 {
        println!("Nothing to update.");
    } else {
        println!("{}", "Skills updated.".green());
    }
    Ok(())
}

async fn download() -> Result<Vec<u8>> {
    println!("Downloading {}...", skills::ZIP_URL);
    let bytes = reqwest::get(skills::ZIP_URL)
        .await
        .context("Failed to download the skills package")?
        .error_for_status()
        .context("Skills package server returned an error")?
        .bytes()
        .await
        .context("Failed to read the skills package")?;
    Ok(bytes.to_vec())
}

/// Replace `dest` with the archive contents, restoring the previous copy if
/// anything goes wrong. The old directory is moved aside rather than deleted
/// so a failed extraction cannot leave the user with nothing.
fn replace_dir(dest: &Path, archive: &[u8]) -> Result<()> {
    let parent = dest.parent().context("Install path has no parent")?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("Failed to create {}", parent.display()))?;

    let backup = dest.with_extension(format!("bak-{}", chrono::Utc::now().timestamp()));
    let had_previous = dest.exists();
    if had_previous {
        std::fs::rename(dest, &backup)
            .with_context(|| format!("Failed to move aside {}", dest.display()))?;
    }

    match extract_zip(archive, dest) {
        Ok(()) => {
            if had_previous {
                let _ = std::fs::remove_dir_all(&backup);
            }
            Ok(())
        }
        Err(e) => {
            let _ = std::fs::remove_dir_all(dest);
            if had_previous {
                let _ = std::fs::rename(&backup, dest);
            }
            Err(e)
        }
    }
}

/// Extract every entry at the path it carries.
///
/// Entries are written verbatim: the published archive has no top-level
/// directory, so stripping a leading path segment would flatten `references/`
/// into the root and break every link `SKILL.md` makes to it.
fn extract_zip(archive: &[u8], dest: &Path) -> Result<()> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(archive))
        .context("Skills package is not a valid archive")?;

    std::fs::create_dir_all(dest)?;

    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        // `enclosed_name` rejects absolute paths and `..` segments, so a
        // tampered archive cannot write outside the install directory.
        let Some(relative) = entry.enclosed_name() else {
            bail!("Skills package contains an unsafe path: {}", entry.name());
        };
        let out = dest.join(relative);

        if entry.is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut buf = Vec::new();
        entry.read_to_end(&mut buf)?;
        std::fs::write(&out, buf).with_context(|| format!("Failed to write {}", out.display()))?;
    }

    if !dest.join("SKILL.md").exists() {
        bail!("Skills package did not contain SKILL.md");
    }
    Ok(())
}

/// Point `link` at `source`. Windows refuses symlinks without the right
/// privileges, and a copy there is better than a failed install — at the cost
/// of that location needing its own update later, which `update` handles.
fn link_or_copy(source: &Path, link: &Path) -> Result<()> {
    if let Some(parent) = link.parent() {
        std::fs::create_dir_all(parent)?;
    }

    #[cfg(unix)]
    let created = std::os::unix::fs::symlink(source, link);
    #[cfg(windows)]
    let created = std::os::windows::fs::symlink_dir(source, link);

    match created {
        Ok(()) => {
            println!("  {} {}", "linked".green(), link.display());
            Ok(())
        }
        Err(_) => {
            copy_dir(source, link)?;
            println!("  {} {}", "copied".green(), link.display());
            Ok(())
        }
    }
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn archive_with(entries: &[(&str, &str)]) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut w = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            for (name, body) in entries {
                w.start_file(*name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                w.write_all(body.as_bytes()).unwrap();
            }
            w.finish().unwrap();
        }
        buf
    }

    /// The published archive has no top-level directory. Flattening entries
    /// here is what broke references for Desktop-installed users.
    #[test]
    fn nested_entries_keep_their_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("linkly-ai");
        let archive = archive_with(&[
            ("SKILL.md", "# skill"),
            ("references/troubleshooting.md", "# trouble"),
        ]);

        extract_zip(&archive, &dest).unwrap();

        assert!(dest.join("SKILL.md").exists());
        assert!(dest.join("references/troubleshooting.md").exists());
    }

    #[test]
    fn an_archive_without_skill_md_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("linkly-ai");
        let archive = archive_with(&[("README.md", "# readme")]);
        assert!(extract_zip(&archive, &dest).is_err());
    }

    #[test]
    fn a_failed_extraction_restores_the_previous_copy() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("linkly-ai");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("SKILL.md"), "old").unwrap();

        // No SKILL.md in the archive, so the replacement must fail.
        let archive = archive_with(&[("README.md", "# readme")]);
        assert!(replace_dir(&dest, &archive).is_err());

        assert_eq!(
            std::fs::read_to_string(dest.join("SKILL.md")).unwrap(),
            "old"
        );
    }

    #[test]
    fn a_git_checkout_is_recognised_and_left_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("linkly-ai");
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        assert!(matches!(classify(&dir), Form::GitCheckout));
    }
}
