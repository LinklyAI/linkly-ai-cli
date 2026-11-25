//! The id this computer goes by when a saved file is matched to the Page made
//! from it before.

use std::path::Path;

const DESKTOP_APP: &str = "ai.linkly.desktop";
const FILE: &str = "pages-device-id";

/// The only setting read from the Desktop's config.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct DesktopConfig {
    device_id: String,
}

/// The Desktop's id, so that files published through either one update the same
/// Pages; without a Desktop, one kept with the CLI's credentials. `None` when
/// none can be kept: the Page is then updated by its url.
pub fn for_pages() -> Option<String> {
    dirs::data_dir()
        .and_then(|data| desktop(&data.join(DESKTOP_APP)))
        .or_else(|| own(&crate::connection::config_dir().ok()?))
}

fn desktop(app_dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(app_dir.join("config.json")).ok()?;
    let config: DesktopConfig = serde_json::from_str(&text).ok()?;
    usable(&config.device_id)
}

fn own(dir: &Path) -> Option<String> {
    let path = dir.join(FILE);
    if let Some(id) = read(&path) {
        return Some(id);
    }
    std::fs::create_dir_all(dir).ok()?;
    // The random part of a temporary file's name is the id.
    let mut file = tempfile::Builder::new()
        .prefix("")
        .rand_bytes(32)
        .tempfile_in(dir)
        .ok()?;
    let id = usable(file.path().file_name()?.to_str()?)?;
    std::io::Write::write_all(&mut file, id.as_bytes()).ok()?;
    match file.persist_noclobber(&path) {
        Ok(_) => Some(id),
        // Another process made one first: that one is kept.
        Err(_) => read(&path),
    }
}

fn read(path: &Path) -> Option<String> {
    usable(&std::fs::read_to_string(path).ok()?)
}

/// Letters, digits and hyphens, as both ids are: it goes in a header.
fn usable(id: &str) -> Option<String> {
    let id = id.trim();
    let plain = id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
    ((8..=64).contains(&id.len()) && plain).then(|| id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_desktops_id_is_read_and_a_broken_config_is_not() {
        let dir = tempfile::tempdir().expect("temp dir");
        let config = dir.path().join("config.json");
        assert_eq!(desktop(dir.path()), None);
        for broken in ["{", r#"{"theme":"dark"}"#, r#"{"deviceId":"a\r\nb: c"}"#] {
            std::fs::write(&config, broken).expect("write");
            assert_eq!(desktop(dir.path()), None, "{broken}");
        }
        let id = "0b9f3c2e-9d5e-4c1a-8f3b-2a6d7e8f9a0b";
        std::fs::write(
            &config,
            format!(r#"{{"cloudApiKey":"lkai_x","deviceId":"{id}"}}"#),
        )
        .expect("write");
        assert_eq!(desktop(dir.path()).as_deref(), Some(id));
    }

    #[test]
    fn the_clis_own_id_stays_the_same_and_nothing_else_is_left_behind() {
        let dir = tempfile::tempdir().expect("temp dir");
        let first = own(dir.path()).expect("an id");
        assert_eq!(first.len(), 32);
        assert_eq!(own(dir.path()).as_deref(), Some(first.as_str()));
        assert_eq!(std::fs::read_dir(dir.path()).expect("list").count(), 1);
        let other = tempfile::tempdir().expect("temp dir");
        assert_ne!(own(other.path()).expect("an id"), first);
    }

    #[test]
    fn processes_that_start_together_agree_on_one_id() {
        let dir = tempfile::tempdir().expect("temp dir");
        let ids: Vec<String> = std::thread::scope(|scope| {
            let threads: Vec<_> = (0..8)
                .map(|_| scope.spawn(|| own(dir.path()).expect("an id")))
                .collect();
            threads
                .into_iter()
                .map(|t| t.join().expect("a thread"))
                .collect()
        });
        assert!(ids.iter().all(|id| id == &ids[0]), "{ids:?}");
    }
}
