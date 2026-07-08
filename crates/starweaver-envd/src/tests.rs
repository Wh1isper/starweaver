#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use starweaver_core::Metadata;
use starweaver_envd_core::{
    CleanupIdleRequest, DEFAULT_ENVIRONMENT_ID, EnvdErrorCode, EnvdService, EnvironmentCapability,
    EnvironmentRequest, EnvironmentStatus, FileReadMode, FileReadRequest, FileWriteRequest,
    OpenEnvironmentRequest,
};
use starweaver_environment::{ShellOutput, VirtualEnvironmentProvider};

use crate::LocalEnvd;

#[tokio::test]
async fn local_envd_wraps_existing_provider_behavior() {
    let provider = Arc::new(
        VirtualEnvironmentProvider::new("test-env")
            .with_file("README.md", "hello")
            .with_shell_output(
                "echo ok",
                ShellOutput {
                    status: 0,
                    stdout: "ok\n".to_string(),
                    stderr: String::new(),
                    metadata: Metadata::default(),
                },
            ),
    );
    let envd = LocalEnvd::new(provider);

    let descriptor = envd
        .open_environment(OpenEnvironmentRequest::default())
        .await
        .unwrap();
    assert_eq!(descriptor.environment_id, DEFAULT_ENVIRONMENT_ID);
    assert_eq!(descriptor.store, "ephemeral");
    assert_eq!(descriptor.status, EnvironmentStatus::Ready);
    assert!(
        descriptor
            .capabilities
            .features
            .contains(&EnvironmentCapability::LifecycleInspect)
    );
    assert!(
        descriptor
            .capabilities
            .features
            .contains(&EnvironmentCapability::LifecyclePrepare)
    );
    assert_eq!(
        envd.prepare_environment(EnvironmentRequest {
            environment_id: DEFAULT_ENVIRONMENT_ID.to_string(),
        })
        .await
        .unwrap()
        .status,
        EnvironmentStatus::Ready
    );
    assert_eq!(
        envd.stop_environment(EnvironmentRequest {
            environment_id: DEFAULT_ENVIRONMENT_ID.to_string(),
        })
        .await
        .unwrap_err()
        .code,
        EnvdErrorCode::Unsupported
    );
    assert_eq!(
        envd.cleanup_idle(CleanupIdleRequest {
            environment_id: DEFAULT_ENVIRONMENT_ID.to_string(),
            older_than_seconds: Some(60),
        })
        .await
        .unwrap_err()
        .code,
        EnvdErrorCode::Unsupported
    );

    let read = envd
        .file_read(FileReadRequest {
            environment_id: DEFAULT_ENVIRONMENT_ID.to_string(),
            path: "README.md".to_string(),
            offset: 0,
            length: None,
            mode: FileReadMode::Text,
        })
        .await
        .unwrap();
    assert_eq!(String::from_utf8(read.bytes).unwrap(), "hello");

    let write = envd
        .file_write(FileWriteRequest {
            environment_id: DEFAULT_ENVIRONMENT_ID.to_string(),
            path: "src/lib.rs".to_string(),
            bytes: b"content".to_vec(),
        })
        .await
        .unwrap();
    assert_eq!(write.state_version, 2);

    let snapshot = envd
        .export_snapshot(EnvironmentRequest {
            environment_id: DEFAULT_ENVIRONMENT_ID.to_string(),
        })
        .await
        .unwrap();
    assert_eq!(snapshot.operations.len(), 1);
    assert_eq!(snapshot.effects.len(), 1);
    assert_eq!(snapshot.mounts[0].backend.kind, "provider");
}

#[tokio::test]
async fn local_envd_operation_history_is_ephemeral() {
    let provider = Arc::new(VirtualEnvironmentProvider::new("test-env"));
    let envd = LocalEnvd::new(provider.clone());
    envd.file_write(FileWriteRequest {
        environment_id: DEFAULT_ENVIRONMENT_ID.to_string(),
        path: "state.txt".to_string(),
        bytes: b"state".to_vec(),
    })
    .await
    .unwrap();

    let snapshot = envd
        .export_snapshot(EnvironmentRequest {
            environment_id: DEFAULT_ENVIRONMENT_ID.to_string(),
        })
        .await
        .unwrap();
    assert_eq!(snapshot.descriptor.store, "ephemeral");
    assert_eq!(snapshot.operations.len(), 1);

    let restarted = LocalEnvd::new(provider);
    let restarted_snapshot = restarted
        .export_snapshot(EnvironmentRequest {
            environment_id: DEFAULT_ENVIRONMENT_ID.to_string(),
        })
        .await
        .unwrap();
    assert_eq!(restarted_snapshot.descriptor.store, "ephemeral");
    assert!(restarted_snapshot.operations.is_empty());
    assert!(restarted_snapshot.effects.is_empty());
}
