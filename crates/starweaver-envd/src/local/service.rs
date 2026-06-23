//! `EnvdService` implementation for `LocalEnvd`.

use async_trait::async_trait;
use serde_json::json;
use starweaver_core::Metadata;
use starweaver_envd_core::{
    CommandRunRequest, CommandRunResult, EnvdError, EnvdResult, EnvdService,
    EnvironmentContextRequest, EnvironmentContextResult, EnvironmentDescriptor, EnvironmentRequest,
    EnvironmentStateSnapshot, FileCopyRequest, FileCreateDirRequest, FileDeleteRequest,
    FileGlobMatch, FileGlobRequest, FileGrepMatch, FileGrepRequest, FileListRequest,
    FileListResult, FileMoveRequest, FileReadMode, FileReadRequest, FileReadResult,
    FileStatRequest, FileWriteRequest, FileWriteResult, FileWriteTmpRequest, FileWriteTmpResult,
    InitializeEnvdRequest, InitializeEnvdResult, MutationResult, OpenEnvironmentRequest,
    ProcessInputRequest, ProcessKillRequest, ProcessListResult, ProcessSignalRequest,
    ProcessSnapshot, ProcessStartRequest, ProcessWaitRequest, ShellReviewContextRequest,
    ShellReviewContextResult, ENVD_PROTOCOL, ENVD_PROTOCOL_VERSION,
};
use starweaver_environment::ShellCommand;

use super::LocalEnvd;
use crate::convert::{
    env_error_to_envd, file_stat_to_envd, glob_options_from_envd, grep_options_from_envd,
    list_options_from_envd, list_result_to_envd, process_to_envd,
};

#[async_trait]
impl EnvdService for LocalEnvd {
    async fn initialize(
        &self,
        _request: InitializeEnvdRequest,
    ) -> EnvdResult<InitializeEnvdResult> {
        Ok(InitializeEnvdResult {
            protocol: ENVD_PROTOCOL.to_string(),
            protocol_version: ENVD_PROTOCOL_VERSION.to_string(),
            service_name: "LocalEnvd".to_string(),
            metadata: Metadata::default(),
        })
    }

    async fn open_environment(
        &self,
        request: OpenEnvironmentRequest,
    ) -> EnvdResult<EnvironmentDescriptor> {
        if let Some(environment_id) = request.environment_id.as_deref() {
            self.ensure_environment(environment_id)?;
        }
        Ok(self.descriptor())
    }

    async fn environment_state(
        &self,
        request: EnvironmentRequest,
    ) -> EnvdResult<EnvironmentStateSnapshot> {
        self.ensure_environment(&request.environment_id)?;
        self.snapshot().await
    }

    async fn file_read(&self, request: FileReadRequest) -> EnvdResult<FileReadResult> {
        self.ensure_environment(&request.environment_id)?;
        let bytes = match request.mode {
            FileReadMode::Text => self
                .provider
                .read_text(&request.path)
                .await
                .map(String::into_bytes),
            FileReadMode::Bytes => {
                self.provider
                    .read_bytes(&request.path, request.offset, request.length)
                    .await
            }
        }
        .map_err(env_error_to_envd)?;
        Ok(FileReadResult { bytes })
    }

    async fn file_write(&self, request: FileWriteRequest) -> EnvdResult<FileWriteResult> {
        self.ensure_environment(&request.environment_id)?;
        let content = String::from_utf8(request.bytes)
            .map_err(|error| EnvdError::invalid_request(error.to_string()))?;
        self.provider
            .write_text(&request.path, &content)
            .await
            .map_err(env_error_to_envd)?;
        let mutation =
            self.record_operation("file.write", Self::operation_metadata("path", request.path))?;
        Ok(FileWriteResult {
            state_version: mutation.state_version,
            operation_id: mutation.operation_id,
        })
    }

    async fn file_create_dir(&self, request: FileCreateDirRequest) -> EnvdResult<MutationResult> {
        self.ensure_environment(&request.environment_id)?;
        self.provider
            .create_dir(&request.path, request.parents)
            .await
            .map_err(env_error_to_envd)?;
        self.record_operation(
            "file.create_dir",
            Self::operation_metadata("path", request.path),
        )
    }

    async fn file_delete(&self, request: FileDeleteRequest) -> EnvdResult<MutationResult> {
        self.ensure_environment(&request.environment_id)?;
        self.provider
            .delete_path(&request.path, request.recursive)
            .await
            .map_err(env_error_to_envd)?;
        self.record_operation(
            "file.delete",
            Self::operation_metadata("path", request.path),
        )
    }

    async fn file_move(&self, request: FileMoveRequest) -> EnvdResult<MutationResult> {
        self.ensure_environment(&request.environment_id)?;
        self.provider
            .move_path(&request.src, &request.dst, request.overwrite)
            .await
            .map_err(env_error_to_envd)?;
        let mut metadata = Metadata::default();
        metadata.insert("src".to_string(), json!(request.src));
        metadata.insert("dst".to_string(), json!(request.dst));
        self.record_operation("file.move", metadata)
    }

    async fn file_copy(&self, request: FileCopyRequest) -> EnvdResult<MutationResult> {
        self.ensure_environment(&request.environment_id)?;
        self.provider
            .copy_path(&request.src, &request.dst, request.overwrite)
            .await
            .map_err(env_error_to_envd)?;
        let mut metadata = Metadata::default();
        metadata.insert("src".to_string(), json!(request.src));
        metadata.insert("dst".to_string(), json!(request.dst));
        self.record_operation("file.copy", metadata)
    }

    async fn file_write_tmp(&self, request: FileWriteTmpRequest) -> EnvdResult<FileWriteTmpResult> {
        self.ensure_environment(&request.environment_id)?;
        let path = self
            .provider
            .write_tmp_file(&request.filename, &request.bytes)
            .await
            .map_err(env_error_to_envd)?;
        let mutation =
            self.record_operation("file.write_tmp", Self::operation_metadata("path", &path))?;
        Ok(FileWriteTmpResult {
            path,
            state_version: mutation.state_version,
            operation_id: mutation.operation_id,
        })
    }

    async fn file_stat(
        &self,
        request: FileStatRequest,
    ) -> EnvdResult<starweaver_envd_core::FileStat> {
        self.ensure_environment(&request.environment_id)?;
        self.provider
            .stat(&request.path)
            .await
            .map(|stat| file_stat_to_envd(&stat))
            .map_err(env_error_to_envd)
    }

    async fn file_list(&self, request: FileListRequest) -> EnvdResult<FileListResult> {
        self.ensure_environment(&request.environment_id)?;
        self.provider
            .list_with_options(&request.path, list_options_from_envd(request.options))
            .await
            .map(list_result_to_envd)
            .map_err(env_error_to_envd)
    }

    async fn file_glob(&self, request: FileGlobRequest) -> EnvdResult<Vec<FileGlobMatch>> {
        self.ensure_environment(&request.environment_id)?;
        self.provider
            .glob(
                &request.path,
                &request.pattern,
                glob_options_from_envd(&request.options),
            )
            .await
            .map(|matches| {
                matches
                    .into_iter()
                    .map(|entry| FileGlobMatch { path: entry.path })
                    .collect()
            })
            .map_err(env_error_to_envd)
    }

    async fn file_grep(&self, request: FileGrepRequest) -> EnvdResult<Vec<FileGrepMatch>> {
        self.ensure_environment(&request.environment_id)?;
        self.provider
            .grep(
                &request.path,
                &request.pattern,
                grep_options_from_envd(request.options),
            )
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
            .map_err(env_error_to_envd)
    }

    async fn command_run(&self, request: CommandRunRequest) -> EnvdResult<CommandRunResult> {
        self.ensure_environment(&request.environment_id)?;
        let output = self
            .provider
            .run_shell(ShellCommand {
                command: request.command.clone(),
                timeout_seconds: request.timeout_seconds,
                cwd: request.cwd,
                environment: request.environment,
            })
            .await
            .map_err(env_error_to_envd)?;
        let mutation = self.record_operation(
            "command.run",
            Metadata::from_iter([("command".to_string(), json!(request.command))]),
        )?;
        Ok(CommandRunResult {
            status: output.status,
            stdout: output.stdout,
            stderr: output.stderr,
            metadata: output.metadata,
            state_version: mutation.state_version,
            operation_id: mutation.operation_id,
        })
    }

    async fn process_start(&self, request: ProcessStartRequest) -> EnvdResult<ProcessSnapshot> {
        self.ensure_environment(&request.environment_id)?;
        self.process_provider()?
            .start_process(ShellCommand {
                command: request.command,
                timeout_seconds: request.timeout_seconds,
                cwd: request.cwd,
                environment: request.environment,
            })
            .await
            .map(process_to_envd)
            .map_err(env_error_to_envd)
    }

    async fn process_wait(&self, request: ProcessWaitRequest) -> EnvdResult<ProcessSnapshot> {
        self.ensure_environment(&request.environment_id)?;
        self.process_provider()?
            .wait_process(&request.process_id, request.timeout_seconds)
            .await
            .map(process_to_envd)
            .map_err(env_error_to_envd)
    }

    async fn process_list(&self, request: EnvironmentRequest) -> EnvdResult<ProcessListResult> {
        self.ensure_environment(&request.environment_id)?;
        self.process_provider()?
            .list_processes()
            .await
            .map(|processes| ProcessListResult {
                processes: processes.into_iter().map(process_to_envd).collect(),
            })
            .map_err(env_error_to_envd)
    }

    async fn process_input(&self, request: ProcessInputRequest) -> EnvdResult<ProcessSnapshot> {
        self.ensure_environment(&request.environment_id)?;
        self.process_provider()?
            .input_process(&request.process_id, &request.text, request.close_stdin)
            .await
            .map(process_to_envd)
            .map_err(env_error_to_envd)
    }

    async fn process_signal(&self, request: ProcessSignalRequest) -> EnvdResult<ProcessSnapshot> {
        self.ensure_environment(&request.environment_id)?;
        self.process_provider()?
            .signal_process(&request.process_id, request.signal)
            .await
            .map(process_to_envd)
            .map_err(env_error_to_envd)
    }

    async fn process_kill(&self, request: ProcessKillRequest) -> EnvdResult<ProcessSnapshot> {
        self.ensure_environment(&request.environment_id)?;
        self.process_provider()?
            .kill_process(&request.process_id)
            .await
            .map(process_to_envd)
            .map_err(env_error_to_envd)
    }

    async fn render_environment_context(
        &self,
        request: EnvironmentContextRequest,
    ) -> EnvdResult<EnvironmentContextResult> {
        self.ensure_environment(&request.environment_id)?;
        self.provider
            .render_environment_context()
            .await
            .map(|text| EnvironmentContextResult { text })
            .map_err(env_error_to_envd)
    }

    async fn shell_review_context(
        &self,
        request: ShellReviewContextRequest,
    ) -> EnvdResult<ShellReviewContextResult> {
        self.ensure_environment(&request.environment_id)?;
        let context = self.provider.shell_review_context();
        Ok(ShellReviewContextResult {
            default_cwd: context.default_cwd,
            allowed_paths: context.allowed_paths,
            shell_platform: context.shell_platform,
            shell_executable: context.shell_executable,
        })
    }

    async fn export_snapshot(
        &self,
        request: EnvironmentRequest,
    ) -> EnvdResult<EnvironmentStateSnapshot> {
        self.ensure_environment(&request.environment_id)?;
        self.snapshot().await
    }
}
