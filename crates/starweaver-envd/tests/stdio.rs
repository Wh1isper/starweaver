//! Standalone envd stdio transport tests.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use starweaver_envd_client::EnvdRpcClient;
use starweaver_envd_core::{
    EnvdService, FileReadMode, FileReadRequest, FileWriteRequest, OpenEnvironmentRequest,
    DEFAULT_ENVIRONMENT_ID,
};

#[tokio::test]
async fn standalone_stdio_round_trips_file_operations() {
    let temp = tempfile::tempdir().expect("temp dir");
    let client = EnvdRpcClient::spawn_stdio(
        env!("CARGO_BIN_EXE_starweaver-envd"),
        [
            "--root".to_string(),
            temp.path().display().to_string(),
            "stdio".to_string(),
        ],
    )
    .unwrap();

    let descriptor = client
        .open_environment(OpenEnvironmentRequest::default())
        .await
        .unwrap();
    assert_eq!(descriptor.environment_id, DEFAULT_ENVIRONMENT_ID);
    assert_eq!(descriptor.store, "ephemeral");

    let write = client
        .file_write(FileWriteRequest {
            environment_id: DEFAULT_ENVIRONMENT_ID.to_string(),
            path: "notes.txt".to_string(),
            bytes: b"stdio ok".to_vec(),
        })
        .await
        .unwrap();
    assert_eq!(write.state_version, 2);

    let read = client
        .file_read(FileReadRequest {
            environment_id: DEFAULT_ENVIRONMENT_ID.to_string(),
            path: "notes.txt".to_string(),
            offset: 0,
            length: None,
            mode: FileReadMode::Text,
        })
        .await
        .unwrap();
    assert_eq!(String::from_utf8(read.bytes).unwrap(), "stdio ok");

    let shutdown = client.shutdown().await.unwrap();
    assert_eq!(shutdown["status"], "shutdown");
}
