#![allow(missing_docs, clippy::unwrap_used)]

use std::process::Command;

fn cli(temp: &tempfile::TempDir) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_starweaver-cli"));
    command.env("STARWEAVER_PROJECT_DIR", temp.path().join(".starweaver"));
    command.env("STARWEAVER_CONFIG_DIR", temp.path().join("global"));
    command
}

#[test]
fn cli_completion_generates_shell_script() {
    let temp = tempfile::tempdir().unwrap();
    let output = cli(&temp).args(["completion", "bash"]).output().unwrap();

    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("starweaver-cli"));
    assert!(stdout.contains("--prompt"));
}

#[test]
fn cli_rejects_ambiguous_session_selectors() {
    let temp = tempfile::tempdir().unwrap();
    let output = cli(&temp)
        .args(["run", "hello", "--session", "session_a", "--new-session"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("cannot be used with"));
}

#[test]
fn top_level_prompt_preserves_hitl_policy_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let output = cli(&temp)
        .args([
            "-p",
            "hello",
            "--hitl",
            "defer",
            "--output",
            "display-jsonl",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let messages = stdout
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert!(messages.iter().all(|message| {
        message["metadata"]["cli_run_policy"]["hitl"].as_str() == Some("defer")
    }));
}
