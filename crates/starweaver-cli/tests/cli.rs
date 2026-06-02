#![allow(missing_docs, clippy::unwrap_used)]

use std::{fs, process::Command, thread};

fn cli(temp: &tempfile::TempDir) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_starweaver-cli"));
    command.env("STARWEAVER_PROJECT_DIR", temp.path().join(".starweaver"));
    command.env("STARWEAVER_CONFIG_DIR", temp.path().join("global"));
    command
}

#[test]
fn cli_diagnostics_prints_sdk_and_version() {
    let temp = tempfile::tempdir().unwrap();
    let output = cli(&temp).arg("diagnostics").output().unwrap();

    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("sdk=starweaver-agent-sdk"));
    assert!(stdout.contains("workspace_version="));
    assert!(stdout.contains("wal=true"));
}

#[test]
fn cli_run_prints_display_messages() {
    let temp = tempfile::tempdir().unwrap();
    let output = cli(&temp).args(["run", "hello"]).output().unwrap();

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
    assert_eq!(messages[0]["schema"], "starweaver.display.v1");
    assert_eq!(messages[0]["type"], "RUN_STARTED");
    for (sequence, message) in messages.iter().enumerate() {
        assert_eq!(message["sequence"].as_u64().unwrap(), sequence as u64);
    }
    let types = messages
        .iter()
        .map(|message| message["type"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(types.contains(&"CHECKPOINT"));
    assert!(types.contains(&"TEXT_MESSAGE_START"));
    assert!(types.contains(&"TEXT_MESSAGE_CONTENT"));
    assert!(types.contains(&"TEXT_MESSAGE_END"));
    assert_eq!(types.last(), Some(&"RUN_FINISHED"));
    let text_messages = messages
        .iter()
        .filter(|message| message["type"] == "TEXT_MESSAGE_CONTENT")
        .collect::<Vec<_>>();
    assert_eq!(text_messages.len(), 1);
    assert_eq!(text_messages[0]["payload"]["delta"], "local echo: hello");

    let session_id = messages[0]["session_id"].as_str().unwrap();
    let run_id = messages[0]["run_id"].as_str().unwrap();
    assert!(messages
        .iter()
        .all(|message| message["run_id"].as_str() == Some(run_id)));
    let raw_stream = temp
        .path()
        .join(".starweaver/store/sessions")
        .join(session_id)
        .join("runs")
        .join(run_id)
        .join("raw.stream.json");
    let raw_records = fs::read_to_string(raw_stream).unwrap();
    let raw_records: serde_json::Value = serde_json::from_str(&raw_records).unwrap();
    let raw_records_array = raw_records.as_array().unwrap();
    assert!(raw_records_array
        .iter()
        .any(|record| record["event"]["kind"] == "model_response"));
    assert!(raw_records_array.iter().any(|record| {
        record["event"]["kind"] == "model_request" && record["event"].get("step").is_some()
    }));

    let compact = temp
        .path()
        .join(".starweaver/store/sessions")
        .join(session_id)
        .join("runs")
        .join(run_id)
        .join("display.compact.json");
    assert!(compact.exists());

    let replay = cli(&temp)
        .args(["session", "replay", session_id, "--run", run_id])
        .output()
        .unwrap();
    assert!(
        replay.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&replay.stderr)
    );
    assert_eq!(String::from_utf8(replay.stdout).unwrap(), stdout);
}

#[test]
fn cli_session_list_and_show_print_local_projection() {
    let temp = tempfile::tempdir().unwrap();
    let run = cli(&temp)
        .args(["-p", "hello", "--output", "silent"])
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&run.stderr)
    );

    let list = cli(&temp).args(["session", "list"]).output().unwrap();
    assert!(
        list.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&list.stderr)
    );
    let stdout = String::from_utf8(list.stdout).unwrap();
    let session: serde_json::Value = serde_json::from_str(stdout.lines().next().unwrap()).unwrap();
    let session_id = session["session_id"].as_str().unwrap();

    let show = cli(&temp)
        .args(["session", "show", session_id])
        .output()
        .unwrap();
    assert!(
        show.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&show.stderr)
    );
    let stdout = String::from_utf8(show.stdout).unwrap();
    assert!(stdout.contains(session_id));
    assert!(stdout.contains("completed"));
}

#[test]
fn cli_config_set_persists_project_config() {
    let temp = tempfile::tempdir().unwrap();
    let set = cli(&temp)
        .args([
            "config",
            "set",
            "trim.current_session_keep_recent_runs",
            "3",
        ])
        .output()
        .unwrap();
    assert!(
        set.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&set.stderr)
    );
    assert_eq!(
        String::from_utf8(set.stdout).unwrap(),
        "trim.current_session_keep_recent_runs=3\n"
    );

    let get = cli(&temp)
        .args(["config", "get", "trim.current_session_keep_recent_runs"])
        .output()
        .unwrap();
    assert!(
        get.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&get.stderr)
    );
    assert_eq!(String::from_utf8(get.stdout).unwrap(), "3\n");
}

#[test]
fn concurrent_cli_runs_append_without_sequence_races() {
    let temp = tempfile::tempdir().unwrap();
    let seed = cli(&temp)
        .args(["-p", "seed", "--output", "silent"])
        .output()
        .unwrap();
    assert!(
        seed.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&seed.stderr)
    );

    let project_dir = temp.path().join(".starweaver");
    let global_dir = temp.path().join("global");
    let handles = (0..6)
        .map(|index| {
            let project_dir = project_dir.clone();
            let global_dir = global_dir.clone();
            thread::spawn(move || {
                Command::new(env!("CARGO_BIN_EXE_starweaver-cli"))
                    .env("STARWEAVER_PROJECT_DIR", project_dir)
                    .env("STARWEAVER_CONFIG_DIR", global_dir)
                    .args([
                        "-p",
                        &format!("run-{index}"),
                        "--continue",
                        "--output",
                        "silent",
                    ])
                    .output()
                    .unwrap()
            })
        })
        .collect::<Vec<_>>();

    for handle in handles {
        let output = handle.join().unwrap();
        assert!(
            output.status.success(),
            "stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let list = cli(&temp).args(["session", "list"]).output().unwrap();
    assert!(
        list.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&list.stderr)
    );
    let stdout = String::from_utf8(list.stdout).unwrap();
    let session: serde_json::Value = serde_json::from_str(stdout.lines().next().unwrap()).unwrap();
    assert_eq!(session["run_count"], 7);
    let session_id = session["session_id"].as_str().unwrap();

    let show = cli(&temp)
        .args(["session", "show", session_id, "--runs", "10"])
        .output()
        .unwrap();
    assert!(
        show.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&show.stderr)
    );
    let sequences = String::from_utf8(show.stdout)
        .unwrap()
        .lines()
        .skip(1)
        .map(|line| {
            serde_json::from_str::<serde_json::Value>(line).unwrap()["sequence_no"]
                .as_u64()
                .unwrap()
        })
        .collect::<Vec<_>>();
    assert_eq!(sequences, vec![1, 2, 3, 4, 5, 6, 7]);
}

#[test]
fn cli_config_set_rejects_negative_unsigned_values() {
    let temp = tempfile::tempdir().unwrap();
    let invalid = cli(&temp)
        .args([
            "config",
            "set",
            "trim.current_session_keep_recent_runs",
            "-1",
        ])
        .output()
        .unwrap();
    assert!(!invalid.status.success());
}

#[test]
fn cli_session_replay_orders_runs_by_session_sequence() {
    let temp = tempfile::tempdir().unwrap();
    for prompt in ["first", "second", "third", "fourth", "fifth", "sixth"] {
        let output = cli(&temp).args(["run", prompt]).output().unwrap();
        assert!(
            output.status.success(),
            "stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let list = cli(&temp).args(["session", "list"]).output().unwrap();
    assert!(
        list.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&list.stderr)
    );
    let stdout = String::from_utf8(list.stdout).unwrap();
    let session: serde_json::Value = serde_json::from_str(stdout.lines().next().unwrap()).unwrap();
    let session_id = session["session_id"].as_str().unwrap();

    let replay = cli(&temp)
        .args(["session", "replay", session_id])
        .output()
        .unwrap();
    assert!(
        replay.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&replay.stderr)
    );
    let replayed_text = String::from_utf8(replay.stdout)
        .unwrap()
        .lines()
        .filter_map(|line| {
            let message: serde_json::Value = serde_json::from_str(line).unwrap();
            if message["type"] == "TEXT_MESSAGE_CONTENT" {
                Some(message["payload"]["delta"].as_str().unwrap().to_string())
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(
        replayed_text,
        vec![
            "local echo: first",
            "local echo: second",
            "local echo: third",
            "local echo: fourth",
            "local echo: fifth",
            "local echo: sixth",
        ]
    );
}
