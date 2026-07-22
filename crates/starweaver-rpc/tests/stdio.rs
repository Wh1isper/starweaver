//! Canonical generated host contract over the standalone stdio process.
#![allow(clippy::expect_used)]

use std::{
    io::{BufRead as _, BufReader, Read as _, Write as _},
    process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};
use starweaver_rpc_core::generated::PROTOCOL_IDENTITY;

mod common;

#[test]
fn eof_closes_active_subscription_and_revokes_connection_attachment() {
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
        &common::initialize_request("req_initialize_eof", "rpc-stdio-eof-test"),
    );
    assert!(initialized.get("result").is_some(), "{initialized}");
    let attached = rpc_round_trip(
        &mut stdin,
        &mut stdout,
        &json!({
            "jsonrpc": "2.0",
            "id": "req_attach_connection",
            "method": "environment.attach",
            "params": {
                "environmentId": "local",
                "idempotencyKey": "stdio-eof-attachment",
                "scope": {"kind": "connection"}
            }
        }),
    );
    let attachment_id = attached["result"]["attachment"]["attachmentId"]
        .as_str()
        .expect("attachment id")
        .to_string();
    let subscribed = rpc_round_trip(
        &mut stdin,
        &mut stdout,
        &json!({
            "jsonrpc": "2.0",
            "id": "req_subscribe_eof",
            "method": "events.subscribe",
            "params": {
                "view": {
                    "optionalFeatures": [],
                    "profile": "operations.v1",
                    "scope": {"kind": "global"}
                }
            }
        }),
    );
    assert!(subscribed.get("result").is_some(), "{subscribed}");

    drop(stdin);
    child.wait_for_exit();

    let database = rusqlite::Connection::open(&store).expect("open durable store");
    let status: String = database
        .query_row(
            "SELECT status FROM environment_attachment_records WHERE attachment_id = ?1",
            [&attachment_id],
            |row| row.get(0),
        )
        .expect("connection attachment record");
    assert_eq!(status, "detached");
}

#[test]
#[allow(clippy::similar_names, clippy::too_many_lines)]
fn notification_stdout_failure_fails_closed_the_whole_stdio_transport() {
    let temp = tempfile::tempdir().expect("temp dir");
    let store = temp.path().join("starweaver.sqlite");
    let command = || {
        let mut command = Command::new(env!("CARGO_BIN_EXE_starweaver-rpc"));
        command
            .current_dir(temp.path())
            .env("STARWEAVER_CONFIG_DIR", temp.path())
            .arg("--store")
            .arg(&store)
            .arg("stdio");
        command
    };

    let mut subscriber = ChildGuard::spawn(&mut command());
    let mut subscriber_stdin = subscriber.stdin();
    let mut subscriber_stdout = BufReader::new(subscriber.stdout());
    let initialized = rpc_round_trip(
        &mut subscriber_stdin,
        &mut subscriber_stdout,
        &common::initialize_request("req_initialize_subscriber", "rpc-stdio-subscriber"),
    );
    assert!(initialized.get("result").is_some(), "{initialized}");
    let attached = rpc_round_trip(
        &mut subscriber_stdin,
        &mut subscriber_stdout,
        &json!({
            "jsonrpc": "2.0",
            "id": "req_attach_subscriber_connection",
            "method": "environment.attach",
            "params": {
                "environmentId": "local",
                "idempotencyKey": "subscriber-connection-attachment",
                "scope": {"kind": "connection"}
            }
        }),
    );
    let attachment_id = attached["result"]["attachment"]["attachmentId"]
        .as_str()
        .expect("subscriber attachment id")
        .to_string();
    let subscribed = rpc_round_trip(
        &mut subscriber_stdin,
        &mut subscriber_stdout,
        &json!({
            "jsonrpc": "2.0",
            "id": "req_subscribe_transport_failure",
            "method": "events.subscribe",
            "params": {
                "view": {
                    "optionalFeatures": [],
                    "profile": "operations.v1",
                    "scope": {"kind": "global"}
                }
            }
        }),
    );
    assert!(subscribed.get("result").is_some(), "{subscribed}");

    let mut publisher = ChildGuard::spawn(&mut command());
    let mut publisher_stdin = publisher.stdin();
    let mut publisher_stdout = BufReader::new(publisher.stdout());
    let publisher_initialized = rpc_round_trip(
        &mut publisher_stdin,
        &mut publisher_stdout,
        &common::initialize_request("req_initialize_publisher", "rpc-stdio-publisher"),
    );
    assert!(
        publisher_initialized.get("result").is_some(),
        "{publisher_initialized}"
    );

    // Keep subscriber stdin open and idle. Closing only stdout's read end makes the next
    // subscription notification hit a real broken pipe, so no request-response failure can mask
    // the notification supervisor path.
    drop(subscriber_stdout);
    let created = rpc_round_trip(
        &mut publisher_stdin,
        &mut publisher_stdout,
        &json!({
            "jsonrpc": "2.0",
            "id": "req_publish_transport_failure_event",
            "method": "session.create",
            "params": {
                "deferredTools": [],
                "idempotencyKey": "notification-transport-failure-event",
                "profile": "default",
                "title": "Trigger subscriber notification"
            }
        }),
    );
    assert!(created.get("result").is_some(), "{created}");

    let (status, stderr) = subscriber.wait_for_failure(Duration::from_secs(8));
    assert_eq!(
        status.code(),
        Some(2),
        "subscriber exited with {status}: {stderr}"
    );
    assert!(
        stderr
            .contains("stdio notification transport failed; connection closed for replay recovery"),
        "unexpected subscriber stderr: {stderr}"
    );
    drop(subscriber_stdin);

    let database = rusqlite::Connection::open(&store).expect("open durable store");
    let status: String = database
        .query_row(
            "SELECT status FROM environment_attachment_records WHERE attachment_id = ?1",
            [&attachment_id],
            |row| row.get(0),
        )
        .expect("subscriber connection attachment record");
    assert_eq!(status, "detached");

    let shutdown = rpc_round_trip(
        &mut publisher_stdin,
        &mut publisher_stdout,
        &json!({
            "jsonrpc": "2.0",
            "id": "req_shutdown_publisher",
            "method": "shutdown",
            "params": {"deadlineMs": 5_000}
        }),
    );
    assert_eq!(shutdown["result"]["status"], "shutdown");
    drop(publisher_stdin);
    publisher.wait_for_exit();
}

#[test]
fn stdio_malformed_requests_share_the_http_error_contract() {
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
        &common::initialize_request("req_initialize_malformed_stdio", "rpc-stdio-malformed-test"),
    );
    assert!(initialized.get("result").is_some(), "{initialized}");

    for vector in common::invalid_request_vectors() {
        let response = raw_rpc_round_trip(&mut stdin, &mut stdout, vector.body);
        common::assert_invalid_request_response(&vector, &response);
    }

    let shutdown = rpc_round_trip(
        &mut stdin,
        &mut stdout,
        &json!({
            "jsonrpc": "2.0",
            "id": "req_shutdown_malformed_stdio_test",
            "method": "shutdown",
            "params": {"deadlineMs": 5_000}
        }),
    );
    assert_eq!(shutdown["result"]["status"], "shutdown");
    drop(stdin);
    child.wait_for_exit();
}

#[test]
fn standalone_stdio_process_serves_the_generated_contract() {
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
        &common::initialize_request("req_initialize", "rpc-stdio-test"),
    );
    assert_eq!(initialized["jsonrpc"], "2.0");
    assert_eq!(initialized["id"], "req_initialize");
    assert_eq!(
        initialized["result"]["protocol"]["schemaDigest"],
        PROTOCOL_IDENTITY.schema_digest
    );
    assert!(
        initialized["result"]["supportedFeatures"]
            .as_array()
            .is_some_and(|features| features.iter().any(|value| value == "events.replay")),
        "{initialized}"
    );

    for (index, vector) in common::conformance_vectors().iter().enumerate() {
        let response = rpc_round_trip(
            &mut stdin,
            &mut stdout,
            &json!({
                "jsonrpc": "2.0",
                "id": format!("req_conformance_{index}"),
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
            "id": "req_shutdown",
            "method": "shutdown",
            "params": {"deadlineMs": 5_000}
        }),
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

    fn wait_for_failure(&mut self, timeout: Duration) -> (ExitStatus, String) {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Some(status) = self.child.try_wait().expect("poll child") {
                let mut stderr = String::new();
                self.child
                    .stderr
                    .take()
                    .expect("child stderr")
                    .read_to_string(&mut stderr)
                    .expect("read child stderr");
                return (status, stderr);
            }
            thread::sleep(Duration::from_millis(25));
        }
        panic!("starweaver-rpc did not fail before the deadline");
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
    raw_rpc_round_trip(stdin, stdout, &request.to_string())
}

fn raw_rpc_round_trip(
    stdin: &mut ChildStdin,
    stdout: &mut BufReader<ChildStdout>,
    request: &str,
) -> Value {
    writeln!(stdin, "{request}").expect("write request");
    stdin.flush().expect("flush request");
    let mut line = String::new();
    stdout.read_line(&mut line).expect("read response");
    serde_json::from_str(line.trim()).expect("json rpc response")
}
