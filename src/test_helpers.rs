//! Shared test utilities for CLI tests.
//!
//! Environment variables are process-global, so every test that overrides one
//! — `HOME`, `LINKLY_NO_SKILLS_HINT`, any other — serializes through this one
//! lock. A second lock guarding a different variable would not help: two tests
//! holding different locks still run at the same time, and one of them swapping
//! `HOME` is enough to break the other.

use std::ffi::OsString;
use std::sync::{Mutex, MutexGuard};

pub(crate) static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Take ENV_LOCK even if an earlier test panicked while holding it: the
/// environment is restored by `EnvRestore` regardless, so the lock's data
/// (there is none) cannot be in a bad state.
pub(crate) fn lock_env() -> MutexGuard<'static, ()> {
    ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Variables that redirect where the CLI reads and writes its state. A
/// developer's shell may export them; tests must never follow them into a
/// real config directory or website.
const ISOLATED_VARS: [&str; 3] = ["HOME", "LINKLY_CONFIG_DIR", "LINKLY_WEB_URL"];

/// Restores ISOLATED_VARS on drop, so a panicking test cannot leak them.
struct EnvRestore(Vec<(&'static str, Option<OsString>)>);

impl Drop for EnvRestore {
    fn drop(&mut self) {
        for (name, value) in &self.0 {
            match value {
                Some(v) => std::env::set_var(name, v),
                None => std::env::remove_var(name),
            }
        }
    }
}

pub(crate) fn with_temp_home<F>(test_name: &str, f: F)
where
    F: FnOnce(std::path::PathBuf),
{
    let _guard = lock_env();
    let temp = tempfile::tempdir().expect("failed to create temp dir");
    let home = temp.path().join(test_name);
    std::fs::create_dir_all(&home).expect("failed to create temp home");

    let _restore = EnvRestore(
        ISOLATED_VARS
            .iter()
            .map(|name| (*name, std::env::var_os(name)))
            .collect(),
    );
    std::env::set_var("HOME", &home);
    std::env::remove_var("LINKLY_CONFIG_DIR");
    std::env::remove_var("LINKLY_WEB_URL");

    f(home);
}
