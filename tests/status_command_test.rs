use std::net::TcpListener;
use std::process::{Command, Output};

fn unused_loopback_endpoint() -> String {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind an ephemeral loopback port");
    let address = listener.local_addr().expect("read the bound address");
    drop(listener);
    format!("http://{}", address)
}

fn run_linkly(args: &[&str]) -> Output {
    let home = tempfile::tempdir().expect("create an isolated home directory");
    Command::new(env!("CARGO_BIN_EXE_linkly"))
        .args(args)
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("NO_PROXY", "127.0.0.1,localhost")
        .env("no_proxy", "127.0.0.1,localhost")
        .env("HTTPS_PROXY", "http://127.0.0.1:9")
        .env("https_proxy", "http://127.0.0.1:9")
        .output()
        .expect("run the linkly binary")
}

#[test]
fn status_reports_unreachable_without_claiming_desktop_is_stopped() {
    let endpoint = unused_loopback_endpoint();
    let output = run_linkly(&["status", "--endpoint", &endpoint, "--token", "test-token"]);

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Unreachable"), "stderr was: {stderr}");
    assert!(
        stderr.contains("does not prove that Linkly AI Desktop is stopped"),
        "stderr was: {stderr}"
    );
    assert!(
        stderr.contains("Use the Linkly MCP integration if it is configured"),
        "stderr was: {stderr}"
    );
    assert!(
        stderr.contains("outside the network sandbox"),
        "stderr was: {stderr}"
    );
    assert!(
        !stderr.contains("Not running"),
        "stderr should not claim the app is stopped: {stderr}"
    );
}

#[test]
fn status_json_exposes_a_stable_desktop_unreachable_code() {
    let endpoint = unused_loopback_endpoint();
    let output = run_linkly(&[
        "--json",
        "status",
        "--endpoint",
        &endpoint,
        "--token",
        "test-token",
    ]);

    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let envelope: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("status should emit one JSON envelope");
    assert_eq!(envelope["status"], "error");
    assert_eq!(envelope["code"], "desktop_unreachable");
    assert_eq!(envelope["endpoint"], endpoint);
    assert!(
        envelope["message"]
            .as_str()
            .expect("message should be a string")
            .contains("does not prove that Linkly AI Desktop is stopped"),
        "stdout was: {stdout}"
    );
}

#[test]
fn doctor_explains_mcp_and_sandbox_recovery_for_an_unreachable_desktop() {
    let endpoint = unused_loopback_endpoint();
    let output = run_linkly(&["doctor", "--endpoint", &endpoint, "--token", "test-token"]);

    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("does not prove that Linkly AI Desktop is stopped"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("Use the Linkly MCP integration if it is configured"),
        "stdout was: {stdout}"
    );
    assert!(
        stdout.contains("outside the network sandbox"),
        "stdout was: {stdout}"
    );
}
