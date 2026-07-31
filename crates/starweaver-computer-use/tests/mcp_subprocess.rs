#![cfg(feature = "mcp-server")]
#![allow(clippy::option_if_let_else)]

//! Real-process protocol tests for the resource-bounded stdio MCP binary.

use std::process::Stdio;

use serde_json::{Value, json};
use starweaver_computer_use::{MAX_MCP_INPUT_FRAME_BYTES, MAX_MCP_JSON_DEPTH};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    time::{Duration, timeout},
};

const IO_TIMEOUT: Duration = Duration::from_secs(10);

struct Harness {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
}

impl Harness {
    fn spawn() -> Self {
        let spawned = Command::new(env!("CARGO_BIN_EXE_starweaver-computer-use-mcp"))
            .arg("--stdio")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn();
        let mut child = match spawned {
            Ok(child) => child,
            Err(error) => panic!("failed to spawn MCP subprocess: {error}"),
        };
        let stdin = child.stdin.take();
        let Some(stdout) = child.stdout.take() else {
            panic!("MCP subprocess stdout was not piped");
        };
        Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
        }
    }

    async fn write_raw(&mut self, bytes: &[u8]) {
        let Some(stdin) = self.stdin.as_mut() else {
            panic!("MCP subprocess stdin is already closed");
        };
        match timeout(IO_TIMEOUT, stdin.write_all(bytes)).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => panic!("failed writing MCP frame: {error}"),
            Err(error) => panic!("timed out writing MCP frame: {error}"),
        }
    }

    async fn send(&mut self, message: &Value) {
        let mut bytes = match serde_json::to_vec(message) {
            Ok(bytes) => bytes,
            Err(error) => panic!("failed serializing MCP test frame: {error}"),
        };
        bytes.push(b'\n');
        self.write_raw(&bytes).await;
    }

    async fn read(&mut self) -> Value {
        let mut line = String::new();
        match timeout(IO_TIMEOUT, self.stdout.read_line(&mut line)).await {
            Ok(Ok(0)) => panic!("MCP subprocess closed stdout before a response"),
            Ok(Ok(_)) => {}
            Ok(Err(error)) => panic!("failed reading MCP response: {error}"),
            Err(error) => panic!("timed out reading MCP response: {error}"),
        }
        match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(error) => panic!("stdout contained non-protocol bytes ({error}): {line:?}"),
        }
    }

    async fn initialize(&mut self) {
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "starweaver-test", "version": "1"}
            }
        }))
        .await;
        let response = self.read().await;
        assert_eq!(response.get("id"), Some(&json!(1)));
        assert_eq!(
            response.pointer("/result/serverInfo/name"),
            Some(&json!("starweaver-computer-use"))
        );
        self.send(&json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }))
        .await;
    }

    async fn close_and_wait(mut self) {
        self.stdin.take();
        self.wait_for_success("stdin EOF").await;
    }

    #[cfg(unix)]
    async fn signal_and_wait(mut self, signal: &str) {
        let Some(pid) = self.child.id() else {
            panic!("MCP subprocess has no process id");
        };
        let sent = std::process::Command::new("kill")
            .args([signal, &pid.to_string()])
            .status();
        match sent {
            Ok(status) if status.success() => {}
            Ok(status) => panic!("kill {signal} failed with {status}"),
            Err(error) => panic!("failed to invoke kill {signal}: {error}"),
        }
        self.stdin.take();
        self.wait_for_success(signal).await;
    }

    async fn wait_for_success(&mut self, cause: &str) {
        let status = match timeout(IO_TIMEOUT, self.child.wait()).await {
            Ok(Ok(status)) => status,
            Ok(Err(error)) => panic!("failed waiting for MCP subprocess: {error}"),
            Err(error) => panic!("MCP subprocess did not exit after {cause}: {error}"),
        };
        assert!(
            status.success(),
            "MCP subprocess exited with {status} after {cause}"
        );

        let mut trailing = String::new();
        match timeout(IO_TIMEOUT, self.stdout.read_to_string(&mut trailing)).await {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => panic!("failed draining MCP stdout: {error}"),
            Err(error) => panic!("timed out draining MCP stdout: {error}"),
        }
        for line in trailing.lines().filter(|line| !line.is_empty()) {
            if let Err(error) = serde_json::from_str::<Value>(line) {
                panic!("stdout contained non-protocol bytes ({error}): {line:?}");
            }
        }
    }
}

#[tokio::test]
async fn subprocess_initialize_call_cancel_eof_and_stdout_cleanliness() {
    let mut harness = Harness::spawn();
    harness.initialize().await;

    harness
        .send(&json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {"name": "computer_status", "arguments": {}}
        }))
        .await;
    let call = harness.read().await;
    assert_eq!(call.get("id"), Some(&json!(2)));
    assert!(call.get("result").is_some());

    harness
        .send(&json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {"name": "computer_status", "arguments": {}}
        }))
        .await;
    harness
        .send(&json!({
            "jsonrpc": "2.0",
            "method": "notifications/cancelled",
            "params": {"requestId": 3, "reason": "subprocess test"}
        }))
        .await;
    harness
        .send(&json!({"jsonrpc": "2.0", "id": 4, "method": "ping"}))
        .await;

    let mut saw_ping = false;
    for _ in 0..2 {
        let response = harness.read().await;
        if response.get("id") == Some(&json!(4)) {
            saw_ping = true;
            assert!(response.get("result").is_some());
            break;
        }
    }
    assert!(
        saw_ping,
        "server did not remain responsive after cancellation"
    );
    harness.close_and_wait().await;
}

#[cfg(unix)]
#[tokio::test]
async fn subprocess_sigint_and_sigterm_share_checked_clean_shutdown() {
    for signal in ["-INT", "-TERM"] {
        let mut harness = Harness::spawn();
        harness.initialize().await;
        harness.signal_and_wait(signal).await;
    }
}

#[tokio::test]
async fn subprocess_rejects_oversize_and_deep_json_then_recovers() {
    let mut harness = Harness::spawn();
    harness.initialize().await;

    let mut oversize = vec![b'x'; MAX_MCP_INPUT_FRAME_BYTES + 1];
    oversize.push(b'\n');
    harness.write_raw(&oversize).await;
    let response = harness.read().await;
    assert_eq!(
        response.pointer("/error/data/code"),
        Some(&json!("mcp_input_frame_too_large"))
    );

    let nesting = MAX_MCP_JSON_DEPTH + 1;
    let mut deep = format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":9,\"method\":\"ping\",\"params\":{}0",
        "[".repeat(nesting)
    );
    deep.push_str(&"]".repeat(nesting));
    deep.push_str("}\n");
    harness.write_raw(deep.as_bytes()).await;
    let response = harness.read().await;
    assert_eq!(
        response.pointer("/error/data/code"),
        Some(&json!("mcp_json_depth_exceeded"))
    );

    harness
        .send(&json!({"jsonrpc": "2.0", "id": 10, "method": "ping"}))
        .await;
    let ping = harness.read().await;
    assert_eq!(ping.get("id"), Some(&json!(10)));
    assert!(ping.get("result").is_some());
    harness.close_and_wait().await;
}
