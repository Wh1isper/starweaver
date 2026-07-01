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

#[test]
fn standalone_http_process_serves_initialize_and_shutdown() {
    let temp = tempfile::tempdir().expect("temp dir");
    let port = free_port();
    let store = temp.path().join("starweaver.sqlite");
    let mut child = ChildGuard::spawn(
        Command::new(env!("CARGO_BIN_EXE_starweaver-rpc"))
            .current_dir(temp.path())
            .env("HOME", temp.path())
            .arg("--store")
            .arg(&store)
            .arg("http")
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--port")
            .arg(port.to_string()),
    );

    wait_for_health(port, &mut child);
    let initialized = post_rpc(
        port,
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {"clientInfo": {"name": "desktop-test"}}
        }),
    );
    assert_eq!(initialized["jsonrpc"], "2.0");
    assert_eq!(initialized["result"]["capabilities"]["sessions"], true);
    assert_eq!(
        initialized["result"]["capabilities"]["streamSubscribe"],
        false
    );
    assert_eq!(initialized["result"]["capabilities"]["streamReplay"], true);
    assert_eq!(initialized["result"]["capabilities"]["liveDisplay"], false);

    let shutdown = post_rpc(
        port,
        &json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "shutdown",
            "params": {}
        }),
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

fn wait_for_health(port: u16, child: &mut ChildGuard) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Some(status) = child.child.try_wait().expect("poll child") {
            panic!("starweaver-rpc exited before health check: {status}");
        }
        if http_get(port, "/health").is_some_and(|body| body.contains(r#""status":"ok""#)) {
            return;
        }
        thread::sleep(Duration::from_millis(25));
    }
    panic!("starweaver-rpc did not become healthy");
}

fn http_get(port: u16, path: &str) -> Option<String> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    let request = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n");
    stream.write_all(request.as_bytes()).ok()?;
    let mut response = String::new();
    stream.read_to_string(&mut response).ok()?;
    response
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.to_string())
}

fn post_rpc(port: u16, body: &Value) -> Value {
    let body = body.to_string();
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect rpc server");
    let request = format!(
        "POST /rpc HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
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
    serde_json::from_str(body).expect("json rpc response")
}
