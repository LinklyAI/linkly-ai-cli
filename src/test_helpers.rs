//! Shared test utilities for CLI tests.
//!
//! Environment variables are process-global, so every test that overrides one
//! — `HOME`, `LINKLY_NO_SKILLS_HINT`, any other — serializes through this one
//! lock. A second lock guarding a different variable would not help: two tests
//! holding different locks still run at the same time, and one of them swapping
//! `HOME` is enough to break the other.

use std::sync::Mutex;

pub(crate) static ENV_LOCK: Mutex<()> = Mutex::new(());

pub(crate) fn with_temp_home<F>(test_name: &str, f: F)
where
    F: FnOnce(std::path::PathBuf),
{
    let _guard = ENV_LOCK.lock().expect("failed to lock the environment");
    let temp = tempfile::tempdir().expect("failed to create temp dir");
    let home = temp.path().join(test_name);
    std::fs::create_dir_all(&home).expect("failed to create temp home");

    let old_home = std::env::var_os("HOME");
    std::env::set_var("HOME", &home);

    f(home);

    match old_home {
        Some(v) => std::env::set_var("HOME", v),
        None => std::env::remove_var("HOME"),
    }
}
