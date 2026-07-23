//! Envd service adapter for SDK environment provider traits.

mod convert;

use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use serde_json::json;
use starweaver_core::Metadata;
use starweaver_envd_core::{
    CleanupIdleRequest, CommandRunRequest, EnvdService, EnvironmentContextRequest,
    EnvironmentRequest, FileCopyRequest, FileCreateDirRequest, FileDeleteRequest, FileGlobRequest,
    FileGrepRequest, FileListRequest, FileMoveRequest, FileReadMode, FileReadRequest,
    FileStatRequest, FileWriteRequest, FileWriteScratchRequest, ProcessInputRequest,
    ProcessKillRequest, ProcessSignalRequest, ProcessStartRequest, ProcessWaitRequest,
};

use crate::{
    EnvironmentError, EnvironmentLifecycleCapabilities, EnvironmentLifecycleSnapshot,
    EnvironmentLifecycleState, EnvironmentProvider, EnvironmentResult, EnvironmentState,
    FileGlobMatch, FileGlobOptions, FileGrepMatch, FileGrepOptions, FileListOptions,
    FileListResult, FileStat, ProcessShellProvider, ShellCommand, ShellOutput,
    ShellProcessSnapshot, ShellReviewEnvironmentContext,
    path_match_candidates as default_path_match_candidates,
    push_shell_review_context_path_candidates,
};

use convert::{
    envd_error_to_environment, file_glob_options_to_envd, file_grep_options_to_envd,
    file_list_options_to_envd, file_list_result_from_envd, file_stat_from_envd, process_from_envd,
    resource_from_envd,
};

/// Metadata key for envd environment ids embedded in SDK environment states.
pub const ENVD_ENVIRONMENT_ID_KEY: &str = "envd_environment_id";

/// Metadata key for envd implementation kind embedded in SDK environment states.
pub const ENVD_KIND_KEY: &str = "envd_kind";

/// Metadata key for envd store kind embedded in SDK environment states.
pub const ENVD_STORE_KEY: &str = "envd_store";

/// Metadata key for envd state versions embedded in SDK environment states.
pub const ENVD_STATE_VERSION_KEY: &str = "envd_state_version";

/// SDK environment provider backed by an envd service.
#[derive(Clone)]
pub struct EnvdEnvironmentProvider {
    id: String,
    environment_id: String,
    service: Arc<dyn EnvdService>,
    shell_review_context: ShellReviewEnvironmentContext,
}

const fn envd_status_to_lifecycle_state(
    status: &starweaver_envd_core::EnvironmentStatus,
) -> EnvironmentLifecycleState {
    match status {
        starweaver_envd_core::EnvironmentStatus::Pending => EnvironmentLifecycleState::Pending,
        starweaver_envd_core::EnvironmentStatus::Preparing => EnvironmentLifecycleState::Preparing,
        starweaver_envd_core::EnvironmentStatus::Ready
        | starweaver_envd_core::EnvironmentStatus::Open => EnvironmentLifecycleState::Ready,
        starweaver_envd_core::EnvironmentStatus::Running => EnvironmentLifecycleState::Running,
        starweaver_envd_core::EnvironmentStatus::Idle => EnvironmentLifecycleState::Idle,
        starweaver_envd_core::EnvironmentStatus::Stopped
        | starweaver_envd_core::EnvironmentStatus::Closed => EnvironmentLifecycleState::Stopped,
        starweaver_envd_core::EnvironmentStatus::Failed => EnvironmentLifecycleState::Failed,
    }
}

fn envd_capabilities_to_lifecycle(
    capabilities: &starweaver_envd_core::EnvironmentCapabilities,
) -> EnvironmentLifecycleCapabilities {
    EnvironmentLifecycleCapabilities {
        inspect: capabilities
            .features
            .contains(&starweaver_envd_core::EnvironmentCapability::LifecycleInspect),
        prepare: capabilities
            .features
            .contains(&starweaver_envd_core::EnvironmentCapability::LifecyclePrepare),
        stop: capabilities
            .features
            .contains(&starweaver_envd_core::EnvironmentCapability::LifecycleStop),
        cleanup_idle: capabilities
            .features
            .contains(&starweaver_envd_core::EnvironmentCapability::LifecycleCleanupIdle),
    }
}

impl EnvdEnvironmentProvider {
    /// Create an envd-backed provider.
    #[must_use]
    pub fn new(service: Arc<dyn EnvdService>, environment_id: impl Into<String>) -> Self {
        let environment_id = environment_id.into();
        Self {
            id: format!("envd:{environment_id}"),
            environment_id,
            service,
            shell_review_context: ShellReviewEnvironmentContext::default(),
        }
    }

    /// Set the SDK provider id.
    #[must_use]
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    /// Set a sync shell review context cached from the direct local backend.
    #[must_use]
    pub fn with_shell_review_context(mut self, context: ShellReviewEnvironmentContext) -> Self {
        self.shell_review_context = context;
        self
    }

    /// Return the underlying envd service.
    #[must_use]
    pub fn service(&self) -> Arc<dyn EnvdService> {
        self.service.clone()
    }

    /// Return the envd environment id.
    #[must_use]
    pub fn environment_id(&self) -> &str {
        &self.environment_id
    }

    fn environment_request(&self) -> EnvironmentRequest {
        EnvironmentRequest {
            environment_id: self.environment_id.clone(),
        }
    }

    fn lifecycle_snapshot_from_descriptor(
        &self,
        descriptor: starweaver_envd_core::EnvironmentDescriptor,
    ) -> EnvironmentLifecycleSnapshot {
        let mut metadata = descriptor.metadata;
        metadata.insert("envd_kind".to_string(), json!(descriptor.kind));
        metadata.insert("envd_store".to_string(), json!(descriptor.store));
        metadata.insert(
            "envd_state_version".to_string(),
            json!(descriptor.state_version),
        );
        metadata.insert(
            "envd_policy_revision".to_string(),
            json!(descriptor.policy_revision),
        );
        EnvironmentLifecycleSnapshot::ready(&self.id)
            .with_environment_id(descriptor.environment_id)
            .with_state(envd_status_to_lifecycle_state(&descriptor.status))
            .with_capabilities(envd_capabilities_to_lifecycle(&descriptor.capabilities))
            .with_metadata(metadata)
    }
}

#[async_trait]
impl EnvironmentProvider for EnvdEnvironmentProvider {
    fn id(&self) -> &str {
        &self.id
    }

    async fn read_text(&self, path: &str) -> EnvironmentResult<String> {
        let result = self
            .service
            .file_read(FileReadRequest {
                environment_id: self.environment_id.clone(),
                path: path.to_string(),
                offset: 0,
                length: None,
                mode: FileReadMode::Text,
            })
            .await
            .map_err(envd_error_to_environment)?;
        String::from_utf8(result.bytes)
            .map_err(|error| EnvironmentError::Provider(error.to_string()))
    }

    async fn read_bytes(
        &self,
        path: &str,
        offset: usize,
        length: Option<usize>,
    ) -> EnvironmentResult<Vec<u8>> {
        self.service
            .file_read(FileReadRequest {
                environment_id: self.environment_id.clone(),
                path: path.to_string(),
                offset,
                length,
                mode: FileReadMode::Bytes,
            })
            .await
            .map(|result| result.bytes)
            .map_err(envd_error_to_environment)
    }

    async fn write_text(&self, path: &str, content: &str) -> EnvironmentResult<()> {
        self.service
            .file_write(FileWriteRequest {
                environment_id: self.environment_id.clone(),
                path: path.to_string(),
                bytes: content.as_bytes().to_vec(),
            })
            .await
            .map(|_| ())
            .map_err(envd_error_to_environment)
    }

    async fn create_dir(&self, path: &str, parents: bool) -> EnvironmentResult<()> {
        self.service
            .file_create_dir(FileCreateDirRequest {
                environment_id: self.environment_id.clone(),
                path: path.to_string(),
                parents,
            })
            .await
            .map(|_| ())
            .map_err(envd_error_to_environment)
    }

    async fn delete_path(&self, path: &str, recursive: bool) -> EnvironmentResult<()> {
        self.service
            .file_delete(FileDeleteRequest {
                environment_id: self.environment_id.clone(),
                path: path.to_string(),
                recursive,
            })
            .await
            .map(|_| ())
            .map_err(envd_error_to_environment)
    }

    async fn move_path(&self, src: &str, dst: &str, overwrite: bool) -> EnvironmentResult<()> {
        self.service
            .file_move(FileMoveRequest {
                environment_id: self.environment_id.clone(),
                src: src.to_string(),
                dst: dst.to_string(),
                overwrite,
            })
            .await
            .map(|_| ())
            .map_err(envd_error_to_environment)
    }

    async fn copy_path(&self, src: &str, dst: &str, overwrite: bool) -> EnvironmentResult<()> {
        self.service
            .file_copy(FileCopyRequest {
                environment_id: self.environment_id.clone(),
                src: src.to_string(),
                dst: dst.to_string(),
                overwrite,
            })
            .await
            .map(|_| ())
            .map_err(envd_error_to_environment)
    }

    async fn write_scratch_file(
        &self,
        filename: &str,
        content: &[u8],
    ) -> EnvironmentResult<String> {
        self.service
            .file_write_scratch(FileWriteScratchRequest {
                environment_id: self.environment_id.clone(),
                filename: filename.to_string(),
                bytes: content.to_vec(),
            })
            .await
            .map(|result| result.path)
            .map_err(envd_error_to_environment)
    }

    async fn stat(&self, path: &str) -> EnvironmentResult<FileStat> {
        self.service
            .file_stat(FileStatRequest {
                environment_id: self.environment_id.clone(),
                path: path.to_string(),
            })
            .await
            .map(|stat| file_stat_from_envd(&stat))
            .map_err(envd_error_to_environment)
    }

    async fn list(&self, path: &str) -> EnvironmentResult<Vec<String>> {
        self.list_with_options(path, FileListOptions::default())
            .await
            .map(|result| result.entries)
    }

    async fn list_with_options(
        &self,
        path: &str,
        options: FileListOptions,
    ) -> EnvironmentResult<FileListResult> {
        self.service
            .file_list(FileListRequest {
                environment_id: self.environment_id.clone(),
                path: path.to_string(),
                options: file_list_options_to_envd(options),
            })
            .await
            .map(file_list_result_from_envd)
            .map_err(envd_error_to_environment)
    }

    fn path_match_candidates(&self, path: &str) -> Vec<String> {
        let mut candidates = default_path_match_candidates(path);
        push_shell_review_context_path_candidates(
            &mut candidates,
            &self.shell_review_context,
            path,
        );
        candidates
    }

    async fn glob(
        &self,
        path: &str,
        pattern: &str,
        options: FileGlobOptions,
    ) -> EnvironmentResult<Vec<FileGlobMatch>> {
        self.service
            .file_glob(FileGlobRequest {
                environment_id: self.environment_id.clone(),
                path: path.to_string(),
                pattern: pattern.to_string(),
                options: file_glob_options_to_envd(&options),
            })
            .await
            .map(|matches| {
                matches
                    .into_iter()
                    .map(|entry| FileGlobMatch { path: entry.path })
                    .collect()
            })
            .map_err(envd_error_to_environment)
    }

    async fn grep(
        &self,
        path: &str,
        pattern: &str,
        options: FileGrepOptions,
    ) -> EnvironmentResult<Vec<FileGrepMatch>> {
        self.service
            .file_grep(FileGrepRequest {
                environment_id: self.environment_id.clone(),
                path: path.to_string(),
                pattern: pattern.to_string(),
                options: file_grep_options_to_envd(options),
            })
            .await
            .map(|matches| {
                matches
                    .into_iter()
                    .map(|entry| FileGrepMatch {
                        path: entry.path,
                        line_number: entry.line_number,
                        matching_line: entry.matching_line,
                        context: entry.context,
                        context_start_line: entry.context_start_line,
                    })
                    .collect()
            })
            .map_err(envd_error_to_environment)
    }

    async fn run_shell(&self, command: ShellCommand) -> EnvironmentResult<ShellOutput> {
        self.service
            .command_run(CommandRunRequest {
                environment_id: self.environment_id.clone(),
                command: command.command,
                timeout_seconds: command.timeout_seconds,
                cwd: command.cwd,
                environment: command.environment,
            })
            .await
            .map(|result| ShellOutput {
                status: result.status,
                stdout: result.stdout,
                stderr: result.stderr,
                metadata: result.metadata,
            })
            .map_err(envd_error_to_environment)
    }

    fn process_shell_provider(self: Arc<Self>) -> Option<crate::DynProcessShellProvider> {
        Some(self)
    }

    fn shell_review_context(&self) -> ShellReviewEnvironmentContext {
        self.shell_review_context.clone()
    }

    async fn render_environment_context(&self) -> EnvironmentResult<Option<String>> {
        self.service
            .render_environment_context(EnvironmentContextRequest {
                environment_id: self.environment_id.clone(),
            })
            .await
            .map(|result| result.text)
            .map_err(envd_error_to_environment)
    }

    async fn inspect_lifecycle(&self) -> EnvironmentResult<EnvironmentLifecycleSnapshot> {
        self.service
            .environment_state(self.environment_request())
            .await
            .map(|snapshot| self.lifecycle_snapshot_from_descriptor(snapshot.descriptor))
            .map_err(envd_error_to_environment)
    }

    async fn prepare(&self) -> EnvironmentResult<EnvironmentLifecycleSnapshot> {
        self.service
            .prepare_environment(self.environment_request())
            .await
            .map(|descriptor| self.lifecycle_snapshot_from_descriptor(descriptor))
            .map_err(envd_error_to_environment)
    }

    async fn stop(&self) -> EnvironmentResult<EnvironmentLifecycleSnapshot> {
        self.service
            .stop_environment(self.environment_request())
            .await
            .map(|descriptor| self.lifecycle_snapshot_from_descriptor(descriptor))
            .map_err(envd_error_to_environment)
    }

    async fn cleanup_idle(&self) -> EnvironmentResult<EnvironmentLifecycleSnapshot> {
        self.service
            .cleanup_idle(CleanupIdleRequest {
                environment_id: self.environment_id.clone(),
                older_than_seconds: None,
            })
            .await
            .map(|descriptor| self.lifecycle_snapshot_from_descriptor(descriptor))
            .map_err(envd_error_to_environment)
    }

    async fn export_state(&self) -> EnvironmentResult<EnvironmentState> {
        let snapshot = self
            .service
            .export_snapshot(self.environment_request())
            .await
            .map_err(envd_error_to_environment)?;
        let mut state = snapshot
            .metadata
            .get("provider_state")
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .unwrap_or_else(|| EnvironmentState {
                provider_id: self.id.clone(),
                files: BTreeMap::default(),
                resources: snapshot
                    .resources
                    .iter()
                    .cloned()
                    .map(resource_from_envd)
                    .collect(),
                processes: snapshot
                    .processes
                    .iter()
                    .cloned()
                    .map(process_from_envd)
                    .collect(),
                metadata: Metadata::default(),
            });
        state.metadata.insert(
            ENVD_ENVIRONMENT_ID_KEY.to_string(),
            json!(snapshot.descriptor.environment_id),
        );
        state
            .metadata
            .insert(ENVD_KIND_KEY.to_string(), json!(snapshot.descriptor.kind));
        state
            .metadata
            .insert(ENVD_STORE_KEY.to_string(), json!(snapshot.descriptor.store));
        state.metadata.insert(
            ENVD_STATE_VERSION_KEY.to_string(),
            json!(snapshot.descriptor.state_version),
        );
        state.metadata.insert(
            "envd_operation_ids".to_string(),
            json!(
                snapshot
                    .operations
                    .iter()
                    .map(|operation| operation.operation_id.as_str())
                    .collect::<Vec<_>>()
            ),
        );
        state.metadata.insert(
            "envd_effect_ids".to_string(),
            json!(
                snapshot
                    .effects
                    .iter()
                    .map(|effect| effect.effect_id.as_str())
                    .collect::<Vec<_>>()
            ),
        );
        Ok(state)
    }
}

#[async_trait]
impl ProcessShellProvider for EnvdEnvironmentProvider {
    async fn start_process(
        &self,
        command: ShellCommand,
    ) -> EnvironmentResult<ShellProcessSnapshot> {
        self.service
            .process_start(ProcessStartRequest {
                environment_id: self.environment_id.clone(),
                command: command.command,
                timeout_seconds: command.timeout_seconds,
                cwd: command.cwd,
                environment: command.environment,
            })
            .await
            .map(process_from_envd)
            .map_err(envd_error_to_environment)
    }

    async fn wait_process(
        &self,
        process_id: &str,
        timeout_seconds: u64,
    ) -> EnvironmentResult<ShellProcessSnapshot> {
        self.service
            .process_wait(ProcessWaitRequest {
                environment_id: self.environment_id.clone(),
                process_id: process_id.to_string(),
                timeout_seconds,
            })
            .await
            .map(process_from_envd)
            .map_err(envd_error_to_environment)
    }

    async fn list_processes(&self) -> EnvironmentResult<Vec<ShellProcessSnapshot>> {
        self.service
            .process_list(self.environment_request())
            .await
            .map(|result| {
                result
                    .processes
                    .into_iter()
                    .map(process_from_envd)
                    .collect()
            })
            .map_err(envd_error_to_environment)
    }

    async fn input_process(
        &self,
        process_id: &str,
        text: &str,
        close_stdin: bool,
    ) -> EnvironmentResult<ShellProcessSnapshot> {
        self.service
            .process_input(ProcessInputRequest {
                environment_id: self.environment_id.clone(),
                process_id: process_id.to_string(),
                text: text.to_string(),
                close_stdin,
            })
            .await
            .map(process_from_envd)
            .map_err(envd_error_to_environment)
    }

    async fn signal_process(
        &self,
        process_id: &str,
        signal: i32,
    ) -> EnvironmentResult<ShellProcessSnapshot> {
        self.service
            .process_signal(ProcessSignalRequest {
                environment_id: self.environment_id.clone(),
                process_id: process_id.to_string(),
                signal,
            })
            .await
            .map(process_from_envd)
            .map_err(envd_error_to_environment)
    }

    async fn kill_process(&self, process_id: &str) -> EnvironmentResult<ShellProcessSnapshot> {
        self.service
            .process_kill(ProcessKillRequest {
                environment_id: self.environment_id.clone(),
                process_id: process_id.to_string(),
            })
            .await
            .map(process_from_envd)
            .map_err(envd_error_to_environment)
    }
}
