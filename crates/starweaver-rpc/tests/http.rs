//! Standalone RPC process HTTP transport tests.
#![allow(clippy::expect_used)]

use std::{
    io::{Read as _, Write as _},
    net::{TcpListener, TcpStream},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};
use starweaver_rpc_core::generated::PROTOCOL_IDENTITY;

mod common;

const HTTP_TOKEN: &str = "test-token-0123456789abcdef-0123456789abcdef";

#[test]
fn standalone_http_process_serves_initialize_and_shutdown() {
    let temp = tempfile::tempdir().expect("temp dir");
    let port = free_port();
    let store = temp.path().join("starweaver.sqlite");
    let mut child = ChildGuard::spawn(
        Command::new(env!("CARGO_BIN_EXE_starweaver-rpc"))
            .current_dir(temp.path())
            .env("HOME", temp.path())
            .env("STARWEAVER_RPC_TOKEN", HTTP_TOKEN)
            .arg("--store")
            .arg(&store)
            .arg("http")
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--port")
            .arg(port.to_string()),
    );

    wait_for_health(port, &mut child, HTTP_TOKEN);
    let initialized = post_rpc(
        port,
        &common::initialize_request("req_initialize", "rpc-http-test"),
        HTTP_TOKEN,
    );
    assert_eq!(initialized["jsonrpc"], "2.0");
    assert_eq!(initialized["id"], "req_initialize");
    assert_eq!(
        initialized["result"]["protocol"]["schemaDigest"],
        PROTOCOL_IDENTITY.schema_digest
    );

    for (index, vector) in common::conformance_vectors().iter().enumerate() {
        let response = post_rpc(
            port,
            &json!({
                "jsonrpc": "2.0",
                "id": format!("req_conformance_{index}"),
                "method": vector.method,
                "params": vector.params,
            }),
            HTTP_TOKEN,
        );
        common::assert_conformance_response(vector, &response);
    }

    let shutdown = post_rpc(
        port,
        &json!({
            "jsonrpc": "2.0",
            "id": "req_shutdown",
            "method": "shutdown",
            "params": {"deadlineMs": 5_000}
        }),
        HTTP_TOKEN,
    );
    assert_eq!(shutdown["result"]["status"], "shutdown");
    child.wait_for_exit();
}

#[test]
fn authenticated_http_malformed_requests_share_the_stdio_error_contract() {
    let temp = tempfile::tempdir().expect("temp dir");
    let port = free_port();
    let mut child = ChildGuard::spawn(
        Command::new(env!("CARGO_BIN_EXE_starweaver-rpc"))
            .current_dir(temp.path())
            .env("HOME", temp.path())
            .env("STARWEAVER_RPC_TOKEN", HTTP_TOKEN)
            .arg("http")
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--port")
            .arg(port.to_string()),
    );
    wait_for_health(port, &mut child, HTTP_TOKEN);

    for vector in common::invalid_request_vectors() {
        let response = post_rpc_text(port, vector.body, HTTP_TOKEN);
        common::assert_invalid_request_response(&vector, &response);
    }

    let shutdown = post_rpc(
        port,
        &json!({
            "jsonrpc": "2.0",
            "id": "req_shutdown_malformed_http_test",
            "method": "shutdown",
            "params": {"deadlineMs": 5_000}
        }),
        HTTP_TOKEN,
    );
    assert_eq!(shutdown["result"]["status"], "shutdown");
    child.wait_for_exit();
}

struct ChildGuard {
    child: Child,
}

impl ChildGuard {
    fn spawn(command: &mut Command) -> Self {
        let child = command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn starweaver-rpc");
        Self { child }
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

fn free_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0))
        .expect("bind free port")
        .local_addr()
        .expect("local addr")
        .port()
}

fn wait_for_health(port: u16, child: &mut ChildGuard, token: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Some(status) = child.child.try_wait().expect("poll child") {
            panic!("starweaver-rpc exited before health check: {status}");
        }
        if http_get(port, "/health", Some(token))
            .is_some_and(|response| response.contains(r#""status":"ok""#))
        {
            return;
        }
        thread::sleep(Duration::from_millis(25));
    }
    panic!("starweaver-rpc did not become healthy");
}

fn http_get(port: u16, path: &str, token: Option<&str>) -> Option<String> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    let authorization = token.map_or_else(String::new, |token| {
        format!("Authorization: Bearer {token}\r\n")
    });
    let request = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{authorization}\r\n");
    stream.write_all(request.as_bytes()).ok()?;
    let mut response = String::new();
    stream.read_to_string(&mut response).ok()?;
    response
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.to_string())
}

fn post_rpc(port: u16, body: &Value, token: &str) -> Value {
    post_rpc_text(port, &body.to_string(), token)
}

fn post_rpc_text(port: u16, body: &str, token: &str) -> Value {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect rpc server");
    let request = format!(
        "POST /rpc HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).expect("write request");
    let mut response = String::new();
    stream.read_to_string(&mut response).expect("read response");
    let (headers, body) = response.split_once("\r\n\r\n").expect("http response body");
    assert!(
        headers.starts_with("HTTP/1.1 200 OK"),
        "unexpected response: {response}"
    );
    assert!(
        headers.contains("Content-Type: application/json"),
        "unexpected response: {response}"
    );
    serde_json::from_str(body).expect("json rpc response")
}

#[test]
#[allow(clippy::too_many_lines)]
fn http_rejects_missing_credentials_browser_blind_writes_and_insufficient_scopes() {
    let temp = tempfile::tempdir().expect("temp dir");
    let port = free_port();
    let mut child = ChildGuard::spawn(
        Command::new(env!("CARGO_BIN_EXE_starweaver-rpc"))
            .current_dir(temp.path())
            .env("HOME", temp.path())
            .env("STARWEAVER_RPC_TOKEN", HTTP_TOKEN)
            .env("STARWEAVER_RPC_SCOPES", "read")
            .arg("http")
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--port")
            .arg(port.to_string()),
    );
    wait_for_health(port, &mut child, HTTP_TOKEN);

    let body = common::initialize_request("req_initialize", "rpc-http-security-test").to_string();
    let missing_auth = raw_http(
        port,
        &format!(
            "POST /rpc HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ),
    );
    assert!(missing_auth.starts_with("HTTP/1.1 401 Unauthorized"));
    assert!(missing_auth.contains("WWW-Authenticate: Bearer"));

    let invalid_auth = raw_http(
        port,
        &format!(
            "POST /rpc HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer invalid-token-that-is-long-enough-000000\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ),
    );
    assert!(invalid_auth.starts_with("HTTP/1.1 401 Unauthorized"));

    let text_plain = raw_http(
        port,
        &format!(
            "POST /rpc HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {HTTP_TOKEN}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ),
    );
    assert!(text_plain.starts_with("HTTP/1.1 415 Unsupported Media Type"));

    let hostile_host = raw_http(
        port,
        &format!(
            "POST /rpc HTTP/1.1\r\nHost: attacker.example\r\nAuthorization: Bearer {HTTP_TOKEN}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ),
    );
    assert!(hostile_host.starts_with("HTTP/1.1 421 Misdirected Request"));

    let browser_origin = raw_http(
        port,
        &format!(
            "POST /rpc HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: https://attacker.example\r\nAuthorization: Bearer {HTTP_TOKEN}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ),
    );
    assert!(browser_origin.starts_with("HTTP/1.1 403 Forbidden"));

    let malformed = "{";
    let read_only_malformed = raw_http(
        port,
        &format!(
            "POST /rpc HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {HTTP_TOKEN}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{malformed}",
            malformed.len()
        ),
    );
    assert!(read_only_malformed.starts_with("HTTP/1.1 403 Forbidden"));

    let run = json!({
        "jsonrpc": "2.0",
        "id": "req_denied_run",
        "method": "run.start",
        "params": {
            "continuationMode": "preserve",
            "environmentAttachments": [],
            "idempotencyKey": "denied-run-start",
            "input": [{"kind": "text", "text": "must not run"}]
        }
    })
    .to_string();
    let read_only_run = raw_http(
        port,
        &format!(
            "POST /rpc HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {HTTP_TOKEN}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{run}",
            run.len()
        ),
    );
    assert!(read_only_run.starts_with("HTTP/1.1 403 Forbidden"));

    for (method, params) in [
        (
            "environment.health",
            json!({"attachmentId": "attachment_missing"}),
        ),
        (
            "approval.decide",
            json!({
                "approvalId": "approval_missing",
                "decision": "approved",
                "expectedRevision": "1",
                "idempotencyKey": "denied-approval-decision"
            }),
        ),
        (
            "session.delete",
            json!({
                "sessionId": "session_missing",
                "expectedRevision": "1",
                "idempotencyKey": "denied-session-delete"
            }),
        ),
    ] {
        let request_body = json!({
            "jsonrpc": "2.0",
            "id": "req_denied_mutation",
            "method": method,
            "params": params
        })
        .to_string();
        let denied = raw_http(
            port,
            &format!(
                "POST /rpc HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {HTTP_TOKEN}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{request_body}",
                request_body.len()
            ),
        );
        assert!(
            denied.starts_with("HTTP/1.1 403 Forbidden"),
            "read-only token unexpectedly called {method}: {denied}"
        );
    }

    let shutdown = json!({
        "jsonrpc": "2.0",
        "id": "req_denied_shutdown",
        "method": "shutdown",
        "params": {"deadlineMs": 5_000}
    })
    .to_string();
    let read_only_shutdown = raw_http(
        port,
        &format!(
            "POST /rpc HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {HTTP_TOKEN}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{shutdown}",
            shutdown.len()
        ),
    );
    assert!(read_only_shutdown.starts_with("HTTP/1.1 403 Forbidden"));
}

fn raw_http(port: u16, request: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect rpc server");
    stream
        .write_all(request.as_bytes())
        .expect("write raw http request");
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .expect("read raw http response");
    response
}
