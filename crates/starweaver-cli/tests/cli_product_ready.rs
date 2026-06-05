#![allow(missing_docs, clippy::unwrap_used)]

use std::process::Command;

fn cli(temp: &tempfile::TempDir) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_starweaver-cli"));
    command.env("STARWEAVER_PROJECT_DIR", temp.path().join(".starweaver"));
    command.env("STARWEAVER_CONFIG_DIR", temp.path().join("global"));
    command
}

fn silent_value(stdout: &[u8], key: &str) -> String {
    String::from_utf8_lossy(stdout)
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{key}=")))
        .unwrap()
        .to_string()
}

#[test]
fn tui_without_session_renders_ready_state() {
    let temp = tempfile::tempdir().unwrap();
    let tui = cli(&temp).arg("tui").output().unwrap();
    assert!(
        tui.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&tui.stderr)
    );
    let stdout = String::from_utf8(tui.stdout).unwrap();
    assert!(stdout.contains("Welcome to Starweaver"));
    assert!(stdout.contains("status=ready"));
    assert!(stdout.contains("session=none"));
    assert!(stdout.contains("sw cli -p \"hello\""));
    assert!(!stdout.contains("make cli -- -p \"hello\""));
    assert!(!temp.path().join(".starweaver/starweaver.sqlite").exists());
    assert!(!temp.path().join(".starweaver/state.json").exists());
    assert!(!temp.path().join(".starweaver/store").exists());
}

#[test]
fn reset_removes_runtime_state_and_preserves_config() {
    let temp = tempfile::tempdir().unwrap();
    let run = cli(&temp)
        .args(["run", "reset me", "--output", "silent"])
        .output()
        .unwrap();
    assert!(run.status.success());
    assert!(temp.path().join(".starweaver/starweaver.sqlite").exists());
    assert!(temp.path().join(".starweaver/state.json").exists());
    assert!(temp.path().join(".starweaver/store").exists());

    let reset = cli(&temp)
        .args(["reset", "--yes", "--output", "silent"])
        .output()
        .unwrap();
    assert!(
        reset.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&reset.stderr)
    );
    assert!(String::from_utf8(reset.stdout)
        .unwrap()
        .contains("status=reset"));
    assert!(!temp.path().join(".starweaver/starweaver.sqlite").exists());
    assert!(!temp.path().join(".starweaver/state.json").exists());
    assert!(!temp.path().join(".starweaver/store").exists());
    assert!(temp.path().join("global/config.toml").exists());
}

#[test]
fn tui_without_session_stays_clean_after_runs_exist() {
    let temp = tempfile::tempdir().unwrap();
    let run = cli(&temp)
        .args(["run", "do not auto replay", "--output", "silent"])
        .output()
        .unwrap();
    assert!(run.status.success());

    let tui = cli(&temp).arg("tui").output().unwrap();
    assert!(
        tui.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&tui.stderr)
    );
    let stdout = String::from_utf8(tui.stdout).unwrap();
    assert!(stdout.contains("Welcome to Starweaver"));
    assert!(stdout.contains("session=none"));
    assert!(!stdout.contains("local echo: do not auto replay"));
    assert!(!stdout.contains("Starweaver CLI TUI snapshot"));
}

#[test]
fn tui_snapshot_renders_display_replay() {
    let temp = tempfile::tempdir().unwrap();
    let run = cli(&temp)
        .args(["run", "hello tui", "--output", "silent"])
        .output()
        .unwrap();
    assert!(run.status.success());
    let session_id = silent_value(&run.stdout, "session_id");

    let tui = cli(&temp)
        .args(["tui", "--session", &session_id])
        .output()
        .unwrap();
    assert!(
        tui.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&tui.stderr)
    );
    let stdout = String::from_utf8(tui.stdout).unwrap();
    assert!(stdout.contains("Starweaver CLI TUI snapshot"));
    assert!(stdout.contains(&format!("session_id={session_id}")));
    assert!(stdout.contains("Assistant"));
    assert!(stdout.contains("local echo: hello tui"));
}

#[test]
fn approval_commands_list_show_and_decide_records() {
    let temp = tempfile::tempdir().unwrap();
    let run = cli(&temp)
        .args([
            "run",
            "needs approval",
            "--new-session",
            "--profile",
            "approval_model",
            "--hitl",
            "defer",
            "--output",
            "silent",
        ])
        .output()
        .unwrap();
    assert!(run.status.success());
    assert!(String::from_utf8_lossy(&run.stdout).contains("status=waiting"));

    let list = cli(&temp).args(["approval", "list"]).output().unwrap();
    assert!(list.status.success());
    let first: serde_json::Value = serde_json::from_str(
        String::from_utf8(list.stdout)
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    let approval_id = first["approval_id"].as_str().unwrap().to_string();
    assert_eq!(first["status"], "pending");

    let show = cli(&temp)
        .args(["approval", "show", &approval_id])
        .output()
        .unwrap();
    assert!(show.status.success());
    assert!(String::from_utf8(show.stdout)
        .unwrap()
        .contains(&approval_id));

    let approve = cli(&temp)
        .args([
            "approval",
            "approve",
            &approval_id,
            "--reason",
            "ok",
            "--output",
            "silent",
        ])
        .output()
        .unwrap();
    assert!(approve.status.success());
    assert!(String::from_utf8(approve.stdout)
        .unwrap()
        .contains("status=approved"));
}

#[test]
fn deferred_commands_complete_and_resume_waiting_session() {
    let temp = tempfile::tempdir().unwrap();
    let run = cli(&temp)
        .args([
            "run",
            "defer me",
            "--new-session",
            "--profile",
            "deferred_model",
            "--hitl",
            "defer",
            "--output",
            "silent",
        ])
        .output()
        .unwrap();
    assert!(run.status.success());
    let session_id = silent_value(&run.stdout, "session_id");

    let list = cli(&temp).args(["deferred", "list"]).output().unwrap();
    assert!(list.status.success());
    let first: serde_json::Value = serde_json::from_str(
        String::from_utf8(list.stdout)
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    let deferred_id = first["deferred_id"].as_str().unwrap().to_string();
    assert_eq!(first["status"], "waiting");

    let complete = cli(&temp)
        .args([
            "deferred",
            "complete",
            &deferred_id,
            "--result",
            r#"{"ok":true}"#,
            "--output",
            "silent",
        ])
        .output()
        .unwrap();
    assert!(
        complete.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&complete.stderr)
    );
    assert!(String::from_utf8(complete.stdout)
        .unwrap()
        .contains("status=completed"));

    let resume = cli(&temp)
        .args([
            "resume",
            "--session",
            &session_id,
            "--prompt",
            "continue after deferred",
            "--output",
            "silent",
        ])
        .output()
        .unwrap();
    assert!(
        resume.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&resume.stderr)
    );
    let resume_stdout = String::from_utf8(resume.stdout).unwrap();
    assert!(resume_stdout.contains(&format!("session_id={session_id}")));
    assert!(resume_stdout.contains("run_id="));
}
