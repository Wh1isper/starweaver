//! Standalone envd HTTP transport tests.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::{
    io::{BufRead as _, BufReader},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use starweaver_envd_client::EnvdRpcClient;
use starweaver_envd_core::{
    DEFAULT_ENVIRONMENT_ID, EnvdService, FileReadMode, FileReadRequest, FileWriteRequest,
    OpenEnvironmentRequest,
};

const TEST_TOKEN: &str = "envd-test-token";

#[tokio::test]
async fn standalone_http_round_trips_file_operations() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut child = ChildGuard::spawn(
        Command::new(env!("CARGO_BIN_EXE_starweaver-envd"))
            .current_dir(temp.path())
            .arg("--root")
            .arg(temp.path())
            .arg("http")
            .arg("--port")
            .arg("0")
            .arg("--token")
            .arg(TEST_TOKEN),
    );
    let endpoint = child.endpoint();
    let client = EnvdRpcClient::http_with_token(&endpoint, TEST_TOKEN).unwrap();

    let descriptor = client
        .open_environment(OpenEnvironmentRequest::default())
        .await
        .unwrap();
    assert_eq!(descriptor.environment_id, DEFAULT_ENVIRONMENT_ID);

    client
        .file_write(FileWriteRequest {
            environment_id: DEFAULT_ENVIRONMENT_ID.to_string(),
            path: "http.txt".to_string(),
            bytes: b"http ok".to_vec(),
        })
        .await
        .unwrap();
    let read = client
        .file_read(FileReadRequest {
            environment_id: DEFAULT_ENVIRONMENT_ID.to_string(),
            path: "http.txt".to_string(),
            offset: 0,
            length: None,
            mode: FileReadMode::Text,
        })
        .await
        .unwrap();
    assert_eq!(String::from_utf8(read.bytes).unwrap(), "http ok");

    let shutdown = client.shutdown().await.unwrap();
    assert_eq!(shutdown["status"], "shutdown");
    child.wait_for_exit();
}

#[tokio::test]
async fn standalone_http_rejects_missing_or_wrong_token() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut child = ChildGuard::spawn(
        Command::new(env!("CARGO_BIN_EXE_starweaver-envd"))
            .current_dir(temp.path())
            .arg("--root")
            .arg(temp.path())
            .arg("http")
            .arg("--port")
            .arg("0")
            .arg("--token")
            .arg(TEST_TOKEN),
    );
    let endpoint = child.endpoint();

    let missing = EnvdRpcClient::http(&endpoint)
        .unwrap()
        .open_environment(OpenEnvironmentRequest::default())
        .await
        .unwrap_err();
    assert!(missing.to_string().contains("401 Unauthorized"));

    let wrong = EnvdRpcClient::http_with_token(&endpoint, "wrong-token")
        .unwrap()
        .open_environment(OpenEnvironmentRequest::default())
        .await
        .unwrap_err();
    assert!(wrong.to_string().contains("401 Unauthorized"));

    let client = EnvdRpcClient::http_with_token(&endpoint, TEST_TOKEN).unwrap();
    let shutdown = client.shutdown().await.unwrap();
    assert_eq!(shutdown["status"], "shutdown");
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
            .expect("spawn starweaver-envd");
        Self { child }
    }

    fn endpoint(&mut self) -> String {
        let stderr = self.child.stderr.take().expect("child stderr");
        let mut reader = BufReader::new(stderr);
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut line = String::new();
        while Instant::now() < deadline {
            line.clear();
            if reader.read_line(&mut line).expect("read stderr") > 0
                && let Some(endpoint) = line
                    .trim()
                    .strip_prefix("starweaver envd http listening on ")
            {
                return endpoint.to_string();
            }
            thread::sleep(Duration::from_millis(25));
        }
        panic!("starweaver-envd did not print HTTP endpoint");
    }

    fn wait_for_exit(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Some(status) = self.child.try_wait().expect("poll child") {
                assert!(status.success(), "starweaver-envd exited with {status}");
                return;
            }
            thread::sleep(Duration::from_millis(25));
        }
        panic!("starweaver-envd did not exit after shutdown");
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
