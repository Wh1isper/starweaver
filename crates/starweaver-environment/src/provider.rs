//! Provider traits for filesystem, shell, and resource access.

use std::sync::Arc;

use async_trait::async_trait;
use grep_regex::RegexMatcher;

use crate::{
    include_path, list_ignore_match, path_match_candidates, search_text, EnvironmentError,
    EnvironmentResult, EnvironmentState, FileGlobMatch, FileGlobOptions, FileGrepMatch,
    FileGrepOptions, FileListOptions, FileListResult, FileStat, PathGlob, ShellCommand,
    ShellOutput, ShellProcessSnapshot, ShellReviewEnvironmentContext,
};

/// Shared environment provider reference.
pub type DynEnvironmentProvider = Arc<dyn EnvironmentProvider>;

/// Process-capable shell provider extension.
#[async_trait]
pub trait ProcessShellProvider: EnvironmentProvider {
    /// Start a background process and return its first snapshot.
    async fn start_process(&self, command: ShellCommand)
        -> EnvironmentResult<ShellProcessSnapshot>;

    /// Wait for or poll a process by id.
    async fn wait_process(
        &self,
        process_id: &str,
        timeout_seconds: u64,
    ) -> EnvironmentResult<ShellProcessSnapshot>;

    /// List known background process snapshots.
    async fn list_processes(&self) -> EnvironmentResult<Vec<ShellProcessSnapshot>>;

    /// Write input to a background process.
    async fn input_process(
        &self,
        process_id: &str,
        text: &str,
        close_stdin: bool,
    ) -> EnvironmentResult<ShellProcessSnapshot>;

    /// Send a signal to a background process.
    async fn signal_process(
        &self,
        process_id: &str,
        signal: i32,
    ) -> EnvironmentResult<ShellProcessSnapshot>;

    /// Kill and clean up a background process.
    async fn kill_process(&self, process_id: &str) -> EnvironmentResult<ShellProcessSnapshot>;
}

/// Shared process-capable provider reference.
pub type DynProcessShellProvider = Arc<dyn ProcessShellProvider>;

/// Provider boundary used by SDK tools and service runtimes.
#[async_trait]
pub trait EnvironmentProvider: Send + Sync {
    /// Provider identifier.
    fn id(&self) -> &str;

    /// Read a UTF-8 text file.
    async fn read_text(&self, path: &str) -> EnvironmentResult<String>;

    /// Read raw bytes from a file.
    async fn read_bytes(
        &self,
        path: &str,
        offset: usize,
        length: Option<usize>,
    ) -> EnvironmentResult<Vec<u8>>;

    /// Write a UTF-8 text file.
    async fn write_text(&self, path: &str, content: &str) -> EnvironmentResult<()>;

    /// Create a provider-scoped directory.
    async fn create_dir(&self, path: &str, parents: bool) -> EnvironmentResult<()>;

    /// Delete a provider-scoped file or directory.
    async fn delete_path(&self, path: &str, recursive: bool) -> EnvironmentResult<()>;

    /// Move a provider-scoped file or directory.
    async fn move_path(&self, src: &str, dst: &str, overwrite: bool) -> EnvironmentResult<()>;

    /// Copy a provider-scoped file or directory.
    async fn copy_path(&self, src: &str, dst: &str, overwrite: bool) -> EnvironmentResult<()>;

    /// Write a provider-scoped temporary file and return the path that tools can pass back to the model.
    async fn write_tmp_file(&self, filename: &str, content: &[u8]) -> EnvironmentResult<String>;

    /// Return provider-scoped file metadata.
    async fn stat(&self, path: &str) -> EnvironmentResult<FileStat>;

    /// List logical entries under a path.
    async fn list(&self, path: &str) -> EnvironmentResult<Vec<String>>;

    /// List logical entries under a path with filtering and result limits.
    async fn list_with_options(
        &self,
        path: &str,
        options: FileListOptions,
    ) -> EnvironmentResult<FileListResult> {
        let mut entries = self.list(path).await?;
        if !options.ignore_patterns.is_empty() {
            entries.retain(|entry| !list_ignore_match(&options.ignore_patterns, entry));
        }
        let total_entries = entries.len();
        let truncated = options.max_entries > 0 && total_entries > options.max_entries;
        if truncated {
            entries.truncate(options.max_entries);
        }
        Ok(FileListResult {
            entries,
            truncated,
            total_entries,
        })
    }

    /// Return path candidates used for provider-scoped path pattern matching.
    ///
    /// Providers may include both caller-supplied and normalized agent-facing
    /// paths so relaxed view policies can match relative and absolute paths.
    fn path_match_candidates(&self, path: &str) -> Vec<String> {
        path_match_candidates(path)
    }

    /// Match provider-scoped paths with ripgrep-style glob semantics.
    async fn glob(
        &self,
        path: &str,
        pattern: &str,
        options: FileGlobOptions,
    ) -> EnvironmentResult<Vec<FileGlobMatch>> {
        let path_glob = PathGlob::new(pattern)?;
        let mut glob_matches = Vec::new();
        for entry in self.list(path).await? {
            if include_path(&entry, options.include_hidden) && path_glob.is_match(&entry) {
                glob_matches.push(FileGlobMatch { path: entry });
                if options.max_results > 0 && glob_matches.len() >= options.max_results {
                    break;
                }
            }
        }
        Ok(glob_matches)
    }

    /// Search provider-scoped text files with ripgrep regex semantics.
    async fn grep(
        &self,
        path: &str,
        pattern: &str,
        options: FileGrepOptions,
    ) -> EnvironmentResult<Vec<FileGrepMatch>> {
        let matcher = RegexMatcher::new_line_matcher(pattern)
            .map_err(|error| EnvironmentError::InvalidRequest(error.to_string()))?;
        let include = options
            .include
            .clone()
            .unwrap_or_else(|| "**/*".to_string());
        let files = self
            .glob(
                path,
                &include,
                FileGlobOptions {
                    include_hidden: options.include_hidden,
                    include_ignored: options.include_ignored,
                    max_results: 0,
                },
            )
            .await?;
        let mut matches = Vec::new();
        let mut searched_files = 0;
        for file in files {
            if options.max_files > 0 && searched_files >= options.max_files {
                break;
            }
            let Ok(content) = self.read_text(&file.path).await else {
                continue;
            };
            searched_files += 1;
            search_text(
                &file.path,
                &content,
                &matcher,
                options.context_lines,
                options.max_matches_per_file,
                options.max_results,
                &mut matches,
            )?;
            if options.max_results > 0 && matches.len() >= options.max_results {
                break;
            }
        }
        Ok(matches)
    }

    /// Execute a foreground shell command through the provider boundary.
    async fn run_shell(&self, command: ShellCommand) -> EnvironmentResult<ShellOutput>;

    /// Return this provider as a process-capable shell provider when supported.
    ///
    /// This lets SDK sessions attach one environment resource and have foreground
    /// and background shell capabilities discovered from that same provider.
    fn process_shell_provider(self: Arc<Self>) -> Option<DynProcessShellProvider> {
        None
    }

    /// Return compact workspace context for shell command review.
    fn shell_review_context(&self) -> ShellReviewEnvironmentContext {
        ShellReviewEnvironmentContext::default()
    }

    /// Render model-facing environment context.
    async fn render_environment_context(&self) -> EnvironmentResult<Option<String>> {
        Ok(None)
    }

    /// Export provider state for resume.
    async fn export_state(&self) -> EnvironmentResult<EnvironmentState>;
}
