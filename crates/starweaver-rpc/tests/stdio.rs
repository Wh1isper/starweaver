//! Standalone RPC process stdio transport tests.
#![allow(clippy::expect_used)]

use std::{
    io::{BufRead as _, BufReader, Write as _},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

mod common;

#[test]
fn standalone_stdio_process_handles_initialize_and_shutdown() {
    let temp = tempfile::tempdir().expect("temp dir");
    let store = temp.path().join("starweaver.sqlite");
    let mut child = ChildGuard::spawn(
        Command::new(env!("CARGO_BIN_EXE_starweaver-rpc"))
            .current_dir(temp.path())
            .env("STARWEAVER_CONFIG_DIR", temp.path())
            .arg("--store")
            .arg(&store)
            .arg("stdio"),
    );
    let mut stdin = child.stdin();
    let mut stdout = BufReader::new(child.stdout());

    let initialized = rpc_round_trip(
        &mut stdin,
        &mut stdout,
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {"clientInfo": {"name": "rpc-stdio-test"}}
        }),
    );
    assert_eq!(initialized["jsonrpc"], "2.0");
    assert_eq!(initialized["result"]["capabilities"]["sessions"], true);
    assert_eq!(
        initialized["result"]["capabilities"]["streamSubscribe"],
        true
    );
    assert_eq!(initialized["result"]["capabilities"]["steering"], true);
    assert_eq!(
        initialized["result"]["capabilities"]["environmentAttachments"],
        true
    );

    for (index, vector) in common::conformance_vectors()
        .iter()
        .filter(|vector| vector.method != "stream.subscribe")
        .enumerate()
    {
        let response = rpc_round_trip(
            &mut stdin,
            &mut stdout,
            &json!({
                "jsonrpc": "2.0",
                "id": 100 + index,
                "method": vector.method,
                "params": vector.params,
            }),
        );
        common::assert_conformance_response(vector, &response);
    }

    let shutdown = rpc_round_trip(
        &mut stdin,
        &mut stdout,
        &json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "shutdown",
            "params": {}
        }),
    );
    assert_eq!(shutdown["result"]["status"], "shutdown");
    drop(stdin);
    child.wait_for_exit();
}

#[test]
#[allow(clippy::too_many_lines)]
fn standalone_rpc_reads_cli_style_session_from_shared_database() {
    use starweaver_core::{ConversationId, RunId};
    use starweaver_session::{RunRecord, RunStatus};
    use starweaver_stream::{DisplayMessage, DisplayMessageKind, ReplayScope, StreamArchive};

    let temp = tempfile::tempdir().expect("temp dir");
    let store = temp.path().join("starweaver.sqlite");
    let storage = starweaver_storage::SqliteStorage::open(&store).expect("shared storage");
    let session = storage
        .create_session_for_product(
            Some("general".to_string()),
            Some("CLI session visible to RPC host".to_string()),
            Some(temp.path().to_string_lossy().into_owned()),
            Some("cli"),
        )
        .expect("CLI-style session");
    let run_id = RunId::from_string("run_cli_to_rpc_process");
    let mut run = RunRecord::new(
        session.session_id.clone(),
        run_id.clone(),
        ConversationId::new(),
    );
    run.trigger_type = Some("cli".to_string());
    run.status = RunStatus::Completed;
    run.output_preview = Some("persisted CLI output".to_string());
    storage.begin_run(run).expect("CLI-style run");
    let display = DisplayMessage::new(
        1,
        session.session_id.clone(),
        run_id.clone(),
        DisplayMessageKind::RunCompleted,
    )
    .with_preview("persisted CLI output");
    tokio::runtime::Runtime::new()
        .expect("test runtime")
        .block_on(async {
            storage
                .stream_archive()
                .append_display_messages(ReplayScope::run(run_id.as_str()), vec![display])
                .await
                .expect("CLI-style display evidence");
        });
    drop(storage);

    let mut child = ChildGuard::spawn(
        Command::new(env!("CARGO_BIN_EXE_starweaver-rpc"))
            .current_dir(temp.path())
            .env("STARWEAVER_CONFIG_DIR", temp.path())
            .arg("--store")
            .arg(&store)
            .arg("stdio"),
    );
    let mut stdin = child.stdin();
    let mut stdout = BufReader::new(child.stdout());
    let initialized = rpc_round_trip(
        &mut stdin,
        &mut stdout,
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {"clientInfo": {"name": "rpc-stdio-test"}}
        }),
    );
    assert!(initialized.get("error").is_none(), "{initialized}");

    let listed = rpc_round_trip(
        &mut stdin,
        &mut stdout,
        &json!({"jsonrpc": "2.0", "id": 2, "method": "session.list", "params": {}}),
    );
    assert!(
        listed["result"]["sessions"]
            .as_array()
            .expect("session list")
            .iter()
            .any(|value| value["session_id"] == session.session_id.as_str()),
        "{listed}"
    );

    let loaded = rpc_round_trip(
        &mut stdin,
        &mut stdout,
        &json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "session.get",
            "params": {"sessionId": session.session_id.as_str()}
        }),
    );
    assert_eq!(
        loaded["result"]["session"]["title"],
        "CLI session visible to RPC host"
    );
    assert_eq!(loaded["result"]["runs"][0]["run_id"], run_id.as_str());

    let replayed = rpc_round_trip(
        &mut stdin,
        &mut stdout,
        &json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "stream.replay",
            "params": {
                "sessionId": session.session_id.as_str(),
                "runId": run_id.as_str()
            }
        }),
    );
    assert_eq!(
        replayed["result"]["messages"][0]["preview"],
        "persisted CLI output"
    );

    let shutdown = rpc_round_trip(
        &mut stdin,
        &mut stdout,
        &json!({"jsonrpc": "2.0", "id": 5, "method": "shutdown", "params": {}}),
    );
    assert_eq!(shutdown["result"]["status"], "shutdown");
    drop(stdin);
    child.wait_for_exit();
}

struct ChildGuard {
    child: Child,
}

impl ChildGuard {
    fn spawn(command: &mut Command) -> Self {
        let child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn starweaver-rpc");
        Self { child }
    }

    const fn stdin(&mut self) -> ChildStdin {
        self.child.stdin.take().expect("child stdin")
    }

    const fn stdout(&mut self) -> ChildStdout {
        self.child.stdout.take().expect("child stdout")
    }

    fn wait_for_exit(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Some(status) = self.child.try_wait().expect("poll child") {
                assert!(status.success(), "starweaver-rpc exited with {status}");
                return;
            }
            thread::sleep(Duration::from_millis(25));
        }
        panic!("starweaver-rpc did not exit after shutdown");
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn rpc_round_trip(
    stdin: &mut ChildStdin,
    stdout: &mut BufReader<ChildStdout>,
    request: &Value,
) -> Value {
    writeln!(stdin, "{request}").expect("write request");
    stdin.flush().expect("flush request");
    let mut line = String::new();
    stdout.read_line(&mut line).expect("read response");
    serde_json::from_str(line.trim()).expect("json rpc response")
}
