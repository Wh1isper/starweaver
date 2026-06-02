#![allow(missing_docs, clippy::unwrap_used)]

use std::process::Command;

fn env_command(bin: &str, temp: &tempfile::TempDir) -> Command {
    let mut command = Command::new(bin);
    command.env("STARWEAVER_PROJECT_DIR", temp.path().join(".starweaver"));
    command.env("STARWEAVER_CONFIG_DIR", temp.path().join("global"));
    command
}

#[test]
fn starweaver_cli_dispatches_to_starweaver_cli_product() {
    let temp = tempfile::tempdir().unwrap();
    let output = env_command(env!("CARGO_BIN_EXE_starweaver"), &temp)
        .args(["cli", "-p", "hello"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let first: serde_json::Value = serde_json::from_str(stdout.lines().next().unwrap()).unwrap();
    assert_eq!(first["schema"], "starweaver.display.v1");
    assert_eq!(first["type"], "RUN_STARTED");
}

#[test]
fn sw_alias_dispatches_to_same_launcher() {
    let temp = tempfile::tempdir().unwrap();
    let output = env_command(env!("CARGO_BIN_EXE_sw"), &temp)
        .args(["cli", "-p", "hello", "--output", "silent"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("session_id=session_"));
    assert!(stdout.contains("status=completed"));
}

#[test]
fn launcher_version_and_doctor_are_builtin() {
    let temp = tempfile::tempdir().unwrap();
    let version = env_command(env!("CARGO_BIN_EXE_starweaver"), &temp)
        .arg("version")
        .output()
        .unwrap();
    assert!(version.status.success());
    assert_eq!(
        String::from_utf8(version.stdout).unwrap().trim(),
        "starweaver-agent-sdk"
    );

    let doctor = env_command(env!("CARGO_BIN_EXE_sw"), &temp)
        .arg("doctor")
        .output()
        .unwrap();
    assert!(doctor.status.success());
    let stdout = String::from_utf8(doctor.stdout).unwrap();
    assert!(stdout.contains("launcher=starweaver"));
    assert!(stdout.contains("cli=starweaver-cli"));
}

#[test]
fn launcher_update_is_builtin() {
    let temp = tempfile::tempdir().unwrap();
    let update = env_command(env!("CARGO_BIN_EXE_starweaver"), &temp)
        .arg("update")
        .output()
        .unwrap();
    assert!(update.status.success());
    let stdout = String::from_utf8(update.stdout).unwrap();
    assert!(stdout.contains("update=github-release"));
    assert!(stdout.contains("status=manual"));
}
