#![allow(missing_docs, clippy::unwrap_used)]

use std::process::Command;

#[test]
fn cli_diagnostics_prints_sdk_and_version() {
    let output = Command::new(env!("CARGO_BIN_EXE_starweaver-cli"))
        .arg("diagnostics")
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("sdk=starweaver-agent-sdk"));
    assert!(stdout.contains("workspace_version="));
}

#[test]
fn cli_run_prints_local_echo() {
    let output = Command::new(env!("CARGO_BIN_EXE_starweaver-cli"))
        .args(["run", "hello"])
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        "local echo: hello"
    );
}

#[test]
fn cli_session_inspect_prints_local_projection() {
    let output = Command::new(env!("CARGO_BIN_EXE_starweaver-cli"))
        .args(["session", "inspect", "session-1"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("session_id=session-1"));
    assert!(stdout.contains("trace_id=trace-session-1"));
    assert!(stdout.contains("store=in-memory"));
}
