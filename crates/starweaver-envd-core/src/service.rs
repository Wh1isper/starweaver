//! `EnvD` service trait.

use async_trait::async_trait;

use crate::{
    CleanupIdleRequest, CommandRunRequest, CommandRunResult, EnvdResult, EnvironmentContextRequest,
    EnvironmentContextResult, EnvironmentDescriptor, EnvironmentRequest, EnvironmentStateSnapshot,
    FileCopyRequest, FileCreateDirRequest, FileDeleteRequest, FileGlobMatch, FileGlobRequest,
    FileGrepMatch, FileGrepRequest, FileListRequest, FileListResult, FileMoveRequest,
    FileReadRequest, FileReadResult, FileStat, FileStatRequest, FileWriteRequest, FileWriteResult,
    FileWriteTmpRequest, FileWriteTmpResult, InitializeEnvdRequest, InitializeEnvdResult,
    MutationResult, OpenEnvironmentRequest, ProcessInputRequest, ProcessKillRequest,
    ProcessListResult, ProcessSignalRequest, ProcessSnapshot, ProcessStartRequest,
    ProcessWaitRequest, ShellReviewContextRequest, ShellReviewContextResult,
};

/// Runtime-neutral envd service interface.
#[async_trait]
pub trait EnvdService: Send + Sync {
    /// Initialize the envd service.
    async fn initialize(&self, request: InitializeEnvdRequest) -> EnvdResult<InitializeEnvdResult>;

    /// Open or describe an environment.
    async fn open_environment(
        &self,
        request: OpenEnvironmentRequest,
    ) -> EnvdResult<EnvironmentDescriptor>;

    /// Return an environment state snapshot.
    async fn environment_state(
        &self,
        request: EnvironmentRequest,
    ) -> EnvdResult<EnvironmentStateSnapshot>;

    /// Prepare the environment where explicit lifecycle control is supported.
    async fn prepare_environment(
        &self,
        request: EnvironmentRequest,
    ) -> EnvdResult<EnvironmentDescriptor>;

    /// Stop the environment where explicit lifecycle control is supported.
    async fn stop_environment(
        &self,
        request: EnvironmentRequest,
    ) -> EnvdResult<EnvironmentDescriptor>;

    /// Clean up idle environment resources where supported.
    async fn cleanup_idle(&self, request: CleanupIdleRequest) -> EnvdResult<EnvironmentDescriptor>;

    /// Read a file.
    async fn file_read(&self, request: FileReadRequest) -> EnvdResult<FileReadResult>;

    /// Write a file.
    async fn file_write(&self, request: FileWriteRequest) -> EnvdResult<FileWriteResult>;

    /// Create a directory.
    async fn file_create_dir(&self, request: FileCreateDirRequest) -> EnvdResult<MutationResult>;

    /// Delete a path.
    async fn file_delete(&self, request: FileDeleteRequest) -> EnvdResult<MutationResult>;

    /// Move a path.
    async fn file_move(&self, request: FileMoveRequest) -> EnvdResult<MutationResult>;

    /// Copy a path.
    async fn file_copy(&self, request: FileCopyRequest) -> EnvdResult<MutationResult>;

    /// Write a temporary file.
    async fn file_write_tmp(&self, request: FileWriteTmpRequest) -> EnvdResult<FileWriteTmpResult>;

    /// Stat a path.
    async fn file_stat(&self, request: FileStatRequest) -> EnvdResult<FileStat>;

    /// List a directory.
    async fn file_list(&self, request: FileListRequest) -> EnvdResult<FileListResult>;

    /// Glob files.
    async fn file_glob(&self, request: FileGlobRequest) -> EnvdResult<Vec<FileGlobMatch>>;

    /// Grep files.
    async fn file_grep(&self, request: FileGrepRequest) -> EnvdResult<Vec<FileGrepMatch>>;

    /// Run a foreground command.
    async fn command_run(&self, request: CommandRunRequest) -> EnvdResult<CommandRunResult>;

    /// Start a background process.
    async fn process_start(&self, request: ProcessStartRequest) -> EnvdResult<ProcessSnapshot>;

    /// Wait for or poll a background process.
    async fn process_wait(&self, request: ProcessWaitRequest) -> EnvdResult<ProcessSnapshot>;

    /// List background processes.
    async fn process_list(&self, request: EnvironmentRequest) -> EnvdResult<ProcessListResult>;

    /// Send input to a background process.
    async fn process_input(&self, request: ProcessInputRequest) -> EnvdResult<ProcessSnapshot>;

    /// Send a signal to a background process.
    async fn process_signal(&self, request: ProcessSignalRequest) -> EnvdResult<ProcessSnapshot>;

    /// Kill a background process.
    async fn process_kill(&self, request: ProcessKillRequest) -> EnvdResult<ProcessSnapshot>;

    /// Render environment context.
    async fn render_environment_context(
        &self,
        request: EnvironmentContextRequest,
    ) -> EnvdResult<EnvironmentContextResult>;

    /// Return shell review context.
    async fn shell_review_context(
        &self,
        request: ShellReviewContextRequest,
    ) -> EnvdResult<ShellReviewContextResult>;

    /// Export a snapshot.
    async fn export_snapshot(
        &self,
        request: EnvironmentRequest,
    ) -> EnvdResult<EnvironmentStateSnapshot>;
}
