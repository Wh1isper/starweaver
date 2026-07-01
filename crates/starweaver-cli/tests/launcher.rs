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
        .args(["cli", "-p", "hello", "--output", "display-jsonl"])
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
    assert_eq!(first["type"], "RUN_QUEUED");
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
fn sw_root_prints_help_and_direct_cli_prompt_works() {
    let temp = tempfile::tempdir().unwrap();
    let help = env_command(env!("CARGO_BIN_EXE_sw"), &temp)
        .output()
        .unwrap();
    assert!(
        help.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&help.stderr)
    );
    let stdout = String::from_utf8(help.stdout).unwrap();
    assert!(stdout.contains("Starweaver product launcher"));
    assert!(stdout.contains("Usage:"));

    let flag_help = env_command(env!("CARGO_BIN_EXE_sw"), &temp)
        .arg("--help")
        .output()
        .unwrap();
    assert!(flag_help.status.success());
    assert!(
        String::from_utf8(flag_help.stdout)
            .unwrap()
            .contains("Use `sw cli --help`")
    );

    let run = env_command(env!("CARGO_BIN_EXE_sw"), &temp)
        .args(["-p", "hello", "--output", "silent"])
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = String::from_utf8(run.stdout).unwrap();
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
        .env("STARWEAVER_UPDATE_DRY_RUN", "1")
        .env("STARWEAVER_INSTALL_DIR", temp.path().join("bin"))
        .arg("update")
        .output()
        .unwrap();
    assert!(update.status.success());
    let stdout = String::from_utf8(update.stdout).unwrap();
    assert!(stdout.contains("update=github-release"));
    assert!(stdout.contains("target=cli"));
    assert!(stdout.contains("status=dry-run"));
}

#[test]
fn launcher_update_accepts_dry_run_and_force_flags() {
    let temp = tempfile::tempdir().unwrap();
    let update = env_command(env!("CARGO_BIN_EXE_sw"), &temp)
        .env("STARWEAVER_INSTALL_DIR", temp.path().join("bin"))
        .args(["update", "--dry-run", "--force"])
        .output()
        .unwrap();
    assert!(
        update.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&update.stderr)
    );
    let stdout = String::from_utf8(update.stdout).unwrap();
    assert!(stdout.contains("target=cli"));
    assert!(stdout.contains("force=true"));
    assert!(stdout.contains("status=dry-run"));
}

#[test]
fn launcher_update_quotes_install_dir_in_dry_run() {
    let temp = tempfile::tempdir().unwrap();
    let install_dir = temp.path().join("bin with ' quote");
    let update = env_command(env!("CARGO_BIN_EXE_starweaver"), &temp)
        .env("STARWEAVER_UPDATE_DRY_RUN", "1")
        .env("STARWEAVER_INSTALL_DIR", &install_dir)
        .args(["update", "cli"])
        .output()
        .unwrap();
    assert!(
        update.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&update.stderr)
    );
    let stdout = String::from_utf8(update.stdout).unwrap();
    assert!(stdout.contains("target=cli"));
    assert!(stdout.contains("'\\''"));
    assert!(stdout.contains("STARWEAVER_COMPONENTS=cli"));
}
