/// Shared test utilities for CLI tests
///
/// HOME env var is process-global, so all tests that override it
/// must serialize through this single lock.

use std::sync::Mutex;

pub(crate) static HOME_LOCK: Mutex<()> = Mutex::new(());

pub(crate) fn with_temp_home<F>(test_name: &str, f: F)
where
    F: FnOnce(std::path::PathBuf),
{
    let _guard = HOME_LOCK.lock().expect("failed to lock HOME");
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
