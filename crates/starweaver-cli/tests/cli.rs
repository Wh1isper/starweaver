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
    let output = cli(&temp)
        .args(["run", "hello", "--output", "display-jsonl"])
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
    assert_eq!(messages[0]["schema"], "starweaver.display.v1");
    assert_eq!(messages[0]["type"], "RUN_QUEUED");
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
    assert!(types.contains(&"COMPACTION_STARTED"));
    assert!(types.contains(&"COMPACTION_COMPLETED"));
    assert!(types.contains(&"RUN_FINISHED"));
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
fn cli_session_list_and_delete_accept_unique_prefix() {
    let temp = tempfile::tempdir().unwrap();
    let run = cli(&temp)
        .args(["-p", "delete me", "--output", "silent"])
        .output()
        .unwrap();
    assert!(run.status.success());
    let list = cli(&temp).args(["session", "list"]).output().unwrap();
    assert!(list.status.success());
    let stdout = String::from_utf8(list.stdout).unwrap();
    let session: serde_json::Value = serde_json::from_str(stdout.lines().next().unwrap()).unwrap();
    let session_id = session["session_id"].as_str().unwrap();
    let prefix = &session_id[..16];
    let delete = cli(&temp)
        .args(["session", "delete", prefix, "--yes", "--output", "silent"])
        .output()
        .unwrap();
    assert!(
        delete.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&delete.stderr)
    );
    assert!(String::from_utf8(delete.stdout)
        .unwrap()
        .contains("status=deleted"));
    let empty = cli(&temp).args(["session", "list"]).output().unwrap();
    assert!(empty.status.success());
    assert!(String::from_utf8(empty.stdout).unwrap().trim().is_empty());
    assert!(!temp
        .path()
        .join(".starweaver/store/sessions")
        .join(session_id)
        .exists());
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
fn cli_prompt_runs_create_new_session_unless_continue_is_requested() {
    let temp = tempfile::tempdir().unwrap();
    let first = cli(&temp)
        .args(["-p", "first", "--output", "silent"])
        .output()
        .unwrap();
    assert!(
        first.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&first.stderr)
    );
    let first_stdout = String::from_utf8(first.stdout).unwrap();
    let first_session_id = first_stdout
        .lines()
        .find_map(|line| line.strip_prefix("session_id="))
        .unwrap()
        .to_string();

    let second = cli(&temp)
        .args(["-p", "second", "--output", "silent"])
        .output()
        .unwrap();
    assert!(
        second.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&second.stderr)
    );
    let second_stdout = String::from_utf8(second.stdout).unwrap();
    let second_session_id = second_stdout
        .lines()
        .find_map(|line| line.strip_prefix("session_id="))
        .unwrap()
        .to_string();
    assert_ne!(first_session_id, second_session_id);

    let continued = cli(&temp)
        .args(["-p", "third", "--continue", "--output", "silent"])
        .output()
        .unwrap();
    assert!(
        continued.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&continued.stderr)
    );
    let continued_stdout = String::from_utf8(continued.stdout).unwrap();
    let continued_session_id = continued_stdout
        .lines()
        .find_map(|line| line.strip_prefix("session_id="))
        .unwrap()
        .to_string();
    assert_eq!(continued_session_id, second_session_id);

    let list = cli(&temp).args(["session", "list"]).output().unwrap();
    assert!(
        list.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&list.stderr)
    );
    let sessions = String::from_utf8(list.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(sessions.len(), 2);
    let first_summary = sessions
        .iter()
        .find(|session| session["session_id"].as_str() == Some(first_session_id.as_str()))
        .unwrap();
    let second_summary = sessions
        .iter()
        .find(|session| session["session_id"].as_str() == Some(second_session_id.as_str()))
        .unwrap();
    assert_eq!(first_summary["run_count"], 1);
    assert_eq!(second_summary["run_count"], 2);
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
    for (index, prompt) in ["first", "second", "third", "fourth", "fifth", "sixth"]
        .into_iter()
        .enumerate()
    {
        let mut command = cli(&temp);
        command.args(["run", prompt]);
        if index > 0 {
            command.arg("--continue");
        }
        let output = command.output().unwrap();
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

#[test]
fn cli_profile_yaml_config_precedence_and_env_defaults_work() {
    let temp = tempfile::tempdir().unwrap();
    let project_dir = temp.path().join(".starweaver");
    let global_dir = temp.path().join("global");
    fs::create_dir_all(project_dir.join("profiles")).unwrap();
    fs::create_dir_all(&global_dir).unwrap();
    fs::write(
        global_dir.join("config.toml"),
        r#"
[general]
default_profile = "general"
default_hitl = "deny"
default_output = "display-jsonl"
[environment]
provider = "virtual"
files_policy = "read_write"
[update]
channel = "beta"
"#,
    )
    .unwrap();
    fs::write(
        project_dir.join("config.toml"),
        r#"
[general]
default_hitl = "defer"
"#,
    )
    .unwrap();
    fs::write(
        project_dir.join("profiles/custom.yaml"),
        r"
name: custom
instructions:
  - Custom deterministic profile.
model:
  model_id: local_echo
toolsets:
  - environment
",
    )
    .unwrap();

    let run = cli(&temp)
        .env("STARWEAVER_OUTPUT", "silent")
        .args(["run", "hello", "--profile", "custom"])
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = String::from_utf8(run.stdout).unwrap();
    assert!(stdout.contains("status=completed"));

    let list = cli(&temp).args(["session", "list"]).output().unwrap();
    assert!(list.status.success());
    let session: serde_json::Value = serde_json::from_str(
        String::from_utf8(list.stdout)
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(session["profile"], "custom");

    let hitl = cli(&temp)
        .args(["config", "get", "general.default_hitl"])
        .output()
        .unwrap();
    assert!(hitl.status.success());
    assert_eq!(String::from_utf8(hitl.stdout).unwrap(), "defer\n");

    let update = cli(&temp)
        .args(["config", "get", "update.channel"])
        .output()
        .unwrap();
    assert!(update.status.success());
    assert_eq!(String::from_utf8(update.stdout).unwrap(), "beta\n");
}

#[test]
#[allow(clippy::too_many_lines)]
fn cli_global_config_set_and_env_hitl_override_work() {
    let temp = tempfile::tempdir().unwrap();
    let set = cli(&temp)
        .args([
            "config",
            "set",
            "--global",
            "general.default_profile",
            "approval_model",
        ])
        .output()
        .unwrap();
    assert!(
        set.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&set.stderr)
    );

    let get = cli(&temp)
        .args(["config", "get", "general.default_profile"])
        .output()
        .unwrap();
    assert!(get.status.success());
    assert_eq!(String::from_utf8(get.stdout).unwrap(), "approval_model\n");

    let run = cli(&temp)
        .env("STARWEAVER_HITL", "fail")
        .args(["run", "needs approval", "--output", "display-jsonl"])
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&run.stderr)
    );
    let messages = String::from_utf8(run.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();
    let types = messages
        .iter()
        .map(|message| message["type"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(types.contains(&"APPROVAL_REQUESTED"));
    assert!(types.contains(&"APPROVAL_RESOLVED"));
    assert!(messages
        .iter()
        .all(|message| { message["metadata"]["cli_run_policy"]["hitl"].as_str() == Some("fail") }));

    let prompt_run = cli(&temp)
        .args([
            "run",
            "needs default deferred approval",
            "--profile",
            "approval_model",
            "--output",
            "display-jsonl",
        ])
        .output()
        .unwrap();
    assert!(
        prompt_run.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&prompt_run.stderr)
    );
    let prompt_messages = String::from_utf8(prompt_run.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();
    let prompt_types = prompt_messages
        .iter()
        .map(|message| message["type"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(prompt_types.contains(&"APPROVAL_REQUESTED"));
    assert!(!prompt_types.contains(&"APPROVAL_RESOLVED"));
    assert!(prompt_messages.iter().all(|message| {
        message["metadata"]["cli_run_policy"]["hitl"].as_str() == Some("defer")
    }));
    let prompt_session_id = prompt_messages[0]["session_id"].as_str().unwrap();
    let prompt_run_id = prompt_messages[0]["run_id"].as_str().unwrap();
    let approvals = cli(&temp)
        .args([
            "approval",
            "list",
            "--session",
            prompt_session_id,
            "--run",
            prompt_run_id,
        ])
        .output()
        .unwrap();
    assert!(
        approvals.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&approvals.stderr)
    );
    let approval_rows = String::from_utf8(approvals.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(approval_rows.len(), 1);
    assert_eq!(approval_rows[0]["status"], "pending");

    let list = cli(&temp).args(["session", "list"]).output().unwrap();
    let session: serde_json::Value = serde_json::from_str(
        String::from_utf8(list.stdout)
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(session["head_success_run_id"], serde_json::Value::Null);
}

#[test]
#[allow(clippy::too_many_lines)]
fn cli_persists_restore_environment_control_flow_and_storage_artifacts() {
    let temp = tempfile::tempdir().unwrap();
    let first = cli(&temp)
        .args(["run", "first", "--output", "silent"])
        .output()
        .unwrap();
    assert!(first.status.success());
    let first_stdout = String::from_utf8(first.stdout).unwrap();
    let session_id = first_stdout
        .lines()
        .find_map(|line| line.strip_prefix("session_id="))
        .unwrap()
        .to_string();
    let first_run_id = first_stdout
        .lines()
        .find_map(|line| line.strip_prefix("run_id="))
        .unwrap()
        .to_string();

    let second = cli(&temp)
        .args(["run", "second", "--continue", "--output", "silent"])
        .output()
        .unwrap();
    assert!(
        second.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&second.stdout),
        String::from_utf8_lossy(&second.stderr)
    );
    let show = cli(&temp)
        .args(["session", "show", &session_id])
        .output()
        .unwrap();
    assert!(show.status.success());
    let rows = String::from_utf8(show.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(rows[2]["restore_from_run_id"], first_run_id);

    let store_root = temp
        .path()
        .join(".starweaver/store/sessions")
        .join(&session_id);
    let first_run_root = store_root.join("runs").join(&first_run_id);
    assert!(first_run_root.join("context.state.json").exists());
    assert!(first_run_root.join("environment.state.json").exists());
    let compact: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(first_run_root.join("display.compact.json")).unwrap(),
    )
    .unwrap();
    assert!(compact["revision"].as_u64().unwrap() > 0);

    let deferred = cli(&temp)
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
    assert!(
        deferred.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&deferred.stderr)
    );
    assert!(String::from_utf8(deferred.stdout)
        .unwrap()
        .contains("status=waiting"));

    let db = temp.path().join(".starweaver/starweaver.sqlite");
    let conn = rusqlite::Connection::open(db).unwrap();
    let approvals: i64 = conn
        .query_row("SELECT COUNT(*) FROM approvals", [], |row| row.get(0))
        .unwrap();
    let deferred_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM deferred_tools", [], |row| row.get(0))
        .unwrap();
    let contexts: i64 = conn
        .query_row("SELECT COUNT(*) FROM context_states", [], |row| row.get(0))
        .unwrap();
    let envs: i64 = conn
        .query_row("SELECT COUNT(*) FROM environment_states", [], |row| {
            row.get(0)
        })
        .unwrap();
    let cursors: i64 = conn
        .query_row("SELECT COUNT(*) FROM stream_cursors", [], |row| row.get(0))
        .unwrap();
    let checkpoints: i64 = conn
        .query_row("SELECT COUNT(*) FROM checkpoints", [], |row| row.get(0))
        .unwrap();
    let file_refs_with_metadata: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM file_refs WHERE checksum IS NOT NULL AND content_type = 'application/json'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(approvals >= 0);
    assert!(deferred_count >= 1);
    assert!(contexts >= 2);
    assert!(envs >= 2);
    assert!(cursors >= 4);
    assert!(checkpoints >= 1);
    assert!(file_refs_with_metadata >= 4);
}

#[test]
fn cli_trim_older_than_dry_run_preserves_recent_and_active_runs() {
    let temp = tempfile::tempdir().unwrap();
    for (index, prompt) in ["one", "two", "three"].into_iter().enumerate() {
        let mut command = cli(&temp);
        command.args(["run", prompt, "--output", "silent"]);
        if index > 0 {
            command.arg("--continue");
        }
        let output = command.output().unwrap();
        assert!(output.status.success());
    }
    let list = cli(&temp).args(["session", "list"]).output().unwrap();
    let session: serde_json::Value = serde_json::from_str(
        String::from_utf8(list.stdout)
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    let session_id = session["session_id"].as_str().unwrap();

    let dry = cli(&temp)
        .args([
            "session",
            "trim",
            "--session",
            session_id,
            "--keep-runs",
            "1",
            "--older-than",
            "0s",
            "--dry-run",
        ])
        .output()
        .unwrap();
    assert!(dry.status.success());
    let report: serde_json::Value = serde_json::from_slice(&dry.stdout).unwrap();
    assert_eq!(report["runs_to_trim"], 2);
    assert_eq!(report["runs_trimmed"], 0);

    let trim = cli(&temp)
        .args([
            "session",
            "trim",
            "--session",
            session_id,
            "--keep-runs",
            "1",
            "--older-than",
            "365d",
        ])
        .output()
        .unwrap();
    assert!(trim.status.success());
    let report: serde_json::Value = serde_json::from_slice(&trim.stdout).unwrap();
    assert_eq!(report["runs_to_trim"], 0);
}

#[test]
fn cli_text_output_profiles_config_init_and_provider_config_work() {
    let temp = tempfile::tempdir().unwrap();

    let text = cli(&temp)
        .args(["run", "hello text", "--output", "text"])
        .output()
        .unwrap();
    assert!(
        text.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&text.stderr)
    );
    assert_eq!(
        String::from_utf8(text.stdout).unwrap(),
        "local echo: hello text\n"
    );

    let init = cli(&temp)
        .args(["config", "init", "--global", "--force"])
        .output()
        .unwrap();
    assert!(
        init.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&init.stderr)
    );
    assert!(temp.path().join("global/config.toml").exists());

    let set = cli(&temp)
        .args([
            "config",
            "set",
            "--global",
            "providers.openai.base_url",
            "https://gateway.example/v1",
        ])
        .output()
        .unwrap();
    assert!(
        set.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&set.stderr)
    );
    let get = cli(&temp)
        .args(["config", "get", "providers.openai.base_url"])
        .output()
        .unwrap();
    assert!(get.status.success());
    assert_eq!(
        String::from_utf8(get.stdout).unwrap(),
        "https://gateway.example/v1\n"
    );

    let profiles = cli(&temp).args(["profile", "list"]).output().unwrap();
    assert!(profiles.status.success());
    let stdout = String::from_utf8(profiles.stdout).unwrap();
    assert!(stdout.contains("\"name\":\"coding\""));
    assert!(stdout.contains("openai:gpt-5"));

    let show = cli(&temp)
        .args(["profile", "show", "coding"])
        .output()
        .unwrap();
    assert!(show.status.success());
    let stdout = String::from_utf8(show.stdout).unwrap();
    assert!(stdout.contains("name: coding"));
    assert!(stdout.contains("model_id: openai:gpt-5"));

    let missing_key = cli(&temp)
        .args(["run", "hello", "--profile", "coding", "--output", "silent"])
        .output()
        .unwrap();
    assert!(!missing_key.status.success());
    assert!(String::from_utf8_lossy(&missing_key.stderr).contains("missing OPENAI_API_KEY"));
}

#[test]
fn cli_provider_missing_or_empty_key_does_not_create_session() {
    let temp = tempfile::tempdir().unwrap();
    let invalid_env = cli(&temp)
        .args(["config", "set", "environment.provider", "remote-for-test"])
        .output()
        .unwrap();
    assert!(!invalid_env.status.success());

    fs::create_dir_all(temp.path().join(".starweaver")).unwrap();
    fs::write(
        temp.path().join(".starweaver/config.toml"),
        r#"
[environment]
provider = "remote-for-test"
"#,
    )
    .unwrap();
    let invalid_env_run = cli(&temp)
        .args(["run", "hello", "--output", "silent"])
        .output()
        .unwrap();
    assert!(!invalid_env_run.status.success());
    assert!(
        String::from_utf8_lossy(&invalid_env_run.stderr).contains("unknown environment provider")
    );
    fs::remove_file(temp.path().join(".starweaver/config.toml")).unwrap();

    let missing_key = cli(&temp)
        .args(["run", "hello", "--profile", "coding", "--output", "silent"])
        .output()
        .unwrap();
    assert!(!missing_key.status.success());
    assert!(String::from_utf8_lossy(&missing_key.stderr).contains("missing OPENAI_API_KEY"));

    let list = cli(&temp).args(["session", "list"]).output().unwrap();
    assert!(
        list.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&list.stderr)
    );
    assert!(String::from_utf8(list.stdout).unwrap().is_empty());

    let empty_key = cli(&temp)
        .env("OPENAI_API_KEY", "   ")
        .args(["run", "hello", "--profile", "coding", "--output", "silent"])
        .output()
        .unwrap();
    assert!(!empty_key.status.success());
    assert!(String::from_utf8_lossy(&empty_key.stderr).contains("missing OPENAI_API_KEY"));

    let ready = cli(&temp)
        .env("OPENAI_API_KEY", "   ")
        .args(["config", "get", "providers.openai.ready"])
        .output()
        .unwrap();
    assert!(ready.status.success());
    assert_eq!(String::from_utf8(ready.stdout).unwrap(), "false\n");

    let invalid_hitl = cli(&temp)
        .args(["config", "set", "general.default_hitl", "maybe"])
        .output()
        .unwrap();
    assert!(!invalid_hitl.status.success());

    let empty_env = cli(&temp)
        .args(["config", "set", "providers.openai.api_key_env", "   "])
        .output()
        .unwrap();
    assert!(!empty_env.status.success());
}
