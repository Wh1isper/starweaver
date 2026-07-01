//! Switchable environment provider handle for active-run host mutations.

use std::sync::{Arc, RwLock};

use async_trait::async_trait;

use crate::{
    DynEnvironmentProvider, DynProcessShellProvider, EnvironmentError, EnvironmentProvider,
    EnvironmentResult, EnvironmentState, FileGlobMatch, FileGlobOptions, FileGrepMatch,
    FileGrepOptions, FileListOptions, FileListResult, FileStat, ProcessShellProvider, ShellCommand,
    ShellOutput, ShellProcessSnapshot, ShellReviewEnvironmentContext,
    path_match_candidates as default_path_match_candidates,
};

/// Current provider target behind a [`SwitchableEnvironmentProvider`].
#[derive(Clone)]
pub struct SwitchableEnvironmentTarget {
    /// Filesystem and foreground shell provider.
    pub provider: DynEnvironmentProvider,
    /// Optional background process provider.
    pub process_provider: Option<DynProcessShellProvider>,
}

impl SwitchableEnvironmentTarget {
    /// Create a switchable target.
    #[must_use]
    pub const fn new(
        provider: DynEnvironmentProvider,
        process_provider: Option<DynProcessShellProvider>,
    ) -> Self {
        Self {
            provider,
            process_provider,
        }
    }
}

/// Environment provider wrapper whose target can be replaced while tools keep
/// the same SDK-facing provider handle.
pub struct SwitchableEnvironmentProvider {
    id: String,
    target: RwLock<SwitchableEnvironmentTarget>,
}

impl SwitchableEnvironmentProvider {
    /// Create a switchable provider.
    #[must_use]
    pub fn new(id: impl Into<String>, target: SwitchableEnvironmentTarget) -> Self {
        Self {
            id: id.into(),
            target: RwLock::new(target),
        }
    }

    /// Replace the current target.
    ///
    /// # Errors
    ///
    /// Returns an environment error if the target lock is poisoned.
    pub fn replace_target(&self, target: SwitchableEnvironmentTarget) -> EnvironmentResult<()> {
        let mut guard = self
            .target
            .write()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
        *guard = target;
        drop(guard);
        Ok(())
    }

    /// Return the current process provider, when the active target supports one.
    ///
    /// # Errors
    ///
    /// Returns an environment error if the target lock is poisoned.
    pub fn process_provider(&self) -> EnvironmentResult<Option<DynProcessShellProvider>> {
        self.current_process_provider()
    }

    fn current_provider(&self) -> EnvironmentResult<DynEnvironmentProvider> {
        self.target
            .read()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))
            .map(|guard| guard.provider.clone())
    }

    fn current_process_provider(&self) -> EnvironmentResult<Option<DynProcessShellProvider>> {
        self.target
            .read()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))
            .map(|guard| guard.process_provider.clone())
    }
}

#[async_trait]
impl EnvironmentProvider for SwitchableEnvironmentProvider {
    fn id(&self) -> &str {
        &self.id
    }

    async fn read_text(&self, path: &str) -> EnvironmentResult<String> {
        self.current_provider()?.read_text(path).await
    }

    async fn read_bytes(
        &self,
        path: &str,
        offset: usize,
        length: Option<usize>,
    ) -> EnvironmentResult<Vec<u8>> {
        self.current_provider()?
            .read_bytes(path, offset, length)
            .await
    }

    async fn write_text(&self, path: &str, content: &str) -> EnvironmentResult<()> {
        self.current_provider()?.write_text(path, content).await
    }

    async fn create_dir(&self, path: &str, parents: bool) -> EnvironmentResult<()> {
        self.current_provider()?.create_dir(path, parents).await
    }

    async fn delete_path(&self, path: &str, recursive: bool) -> EnvironmentResult<()> {
        self.current_provider()?.delete_path(path, recursive).await
    }

    async fn move_path(&self, src: &str, dst: &str, overwrite: bool) -> EnvironmentResult<()> {
        self.current_provider()?
            .move_path(src, dst, overwrite)
            .await
    }

    async fn copy_path(&self, src: &str, dst: &str, overwrite: bool) -> EnvironmentResult<()> {
        self.current_provider()?
            .copy_path(src, dst, overwrite)
            .await
    }

    async fn write_tmp_file(&self, filename: &str, content: &[u8]) -> EnvironmentResult<String> {
        self.current_provider()?
            .write_tmp_file(filename, content)
            .await
    }

    async fn stat(&self, path: &str) -> EnvironmentResult<FileStat> {
        self.current_provider()?.stat(path).await
    }

    async fn list(&self, path: &str) -> EnvironmentResult<Vec<String>> {
        self.current_provider()?.list(path).await
    }

    async fn list_with_options(
        &self,
        path: &str,
        options: FileListOptions,
    ) -> EnvironmentResult<FileListResult> {
        self.current_provider()?
            .list_with_options(path, options)
            .await
    }

    fn path_match_candidates(&self, path: &str) -> Vec<String> {
        self.current_provider().map_or_else(
            |_| default_path_match_candidates(path),
            |provider| provider.path_match_candidates(path),
        )
    }

    async fn glob(
        &self,
        path: &str,
        pattern: &str,
        options: FileGlobOptions,
    ) -> EnvironmentResult<Vec<FileGlobMatch>> {
        self.current_provider()?.glob(path, pattern, options).await
    }

    async fn grep(
        &self,
        path: &str,
        pattern: &str,
        options: FileGrepOptions,
    ) -> EnvironmentResult<Vec<FileGrepMatch>> {
        self.current_provider()?.grep(path, pattern, options).await
    }

    async fn run_shell(&self, command: ShellCommand) -> EnvironmentResult<ShellOutput> {
        self.current_provider()?.run_shell(command).await
    }

    fn process_shell_provider(self: Arc<Self>) -> Option<DynProcessShellProvider> {
        Some(self)
    }

    fn shell_review_context(&self) -> ShellReviewEnvironmentContext {
        self.current_provider().map_or_else(
            |_| ShellReviewEnvironmentContext::default(),
            |provider| provider.shell_review_context(),
        )
    }

    async fn render_environment_context(&self) -> EnvironmentResult<Option<String>> {
        self.current_provider()?.render_environment_context().await
    }

    async fn export_state(&self) -> EnvironmentResult<EnvironmentState> {
        self.current_provider()?.export_state().await
    }
}

#[async_trait]
impl ProcessShellProvider for SwitchableEnvironmentProvider {
    async fn start_process(
        &self,
        command: ShellCommand,
    ) -> EnvironmentResult<ShellProcessSnapshot> {
        self.current_process_provider()?
            .ok_or_else(shell_unavailable)?
            .start_process(command)
            .await
    }

    async fn wait_process(
        &self,
        process_id: &str,
        timeout_seconds: u64,
    ) -> EnvironmentResult<ShellProcessSnapshot> {
        self.current_process_provider()?
            .ok_or_else(shell_unavailable)?
            .wait_process(process_id, timeout_seconds)
            .await
    }

    async fn list_processes(&self) -> EnvironmentResult<Vec<ShellProcessSnapshot>> {
        self.current_process_provider()?
            .ok_or_else(shell_unavailable)?
            .list_processes()
            .await
    }

    async fn input_process(
        &self,
        process_id: &str,
        text: &str,
        close_stdin: bool,
    ) -> EnvironmentResult<ShellProcessSnapshot> {
        self.current_process_provider()?
            .ok_or_else(shell_unavailable)?
            .input_process(process_id, text, close_stdin)
            .await
    }

    async fn signal_process(
        &self,
        process_id: &str,
        signal: i32,
    ) -> EnvironmentResult<ShellProcessSnapshot> {
        self.current_process_provider()?
            .ok_or_else(shell_unavailable)?
            .signal_process(process_id, signal)
            .await
    }

    async fn kill_process(&self, process_id: &str) -> EnvironmentResult<ShellProcessSnapshot> {
        self.current_process_provider()?
            .ok_or_else(shell_unavailable)?
            .kill_process(process_id)
            .await
    }
}

fn shell_unavailable() -> EnvironmentError {
    EnvironmentError::InvalidRequest("active environment shell provider is unavailable".to_string())
}
