//! Environment provider abstractions for filesystem, shell, and resource access.

use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, Read},
    path::{Component, Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant, UNIX_EPOCH},
};

use async_trait::async_trait;
use globset::{GlobBuilder, GlobMatcher};
use grep_matcher::Matcher;
use grep_regex::RegexMatcher;
use grep_searcher::{
    BinaryDetection, Searcher, SearcherBuilder, Sink, SinkContext, SinkFinish, SinkMatch,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use starweaver_core::{Metadata, XmlWriter};
use thiserror::Error;

/// Shared environment provider reference.
pub type DynEnvironmentProvider = Arc<dyn EnvironmentProvider>;

const DEFAULT_INSTRUCTIONS_SKIP_DIRS: &[&str] = &["node_modules", ".git", ".venv", "__pycache__"];
const DEFAULT_INSTRUCTIONS_MAX_DEPTH: usize = 3;
const DEFAULT_TMP_DIR: &str = ".starweaver/tmp";
const LOCAL_TMP_DIR_PREFIX: &str = "starweaver-";

/// Environment operation failure.
#[derive(Debug, Error)]
pub enum EnvironmentError {
    /// Access was denied by policy.
    #[error("environment access denied: {0}")]
    AccessDenied(String),
    /// Requested resource was not found.
    #[error("environment resource not found: {0}")]
    NotFound(String),
    /// Input was invalid for this provider.
    #[error("invalid environment request: {0}")]
    InvalidRequest(String),
    /// Provider execution failed.
    #[error("environment provider failed: {0}")]
    Provider(String),
}

/// Result alias for environment provider operations.
pub type EnvironmentResult<T> = Result<T, EnvironmentError>;

/// Filesystem policy for provider-backed tools.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct FilePolicy {
    /// Whether read operations are allowed.
    pub allow_read: bool,
    /// Whether write operations are allowed.
    pub allow_write: bool,
    /// Allowed logical path prefixes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_prefixes: Vec<String>,
}

impl FilePolicy {
    /// Policy allowing read-only access to all provider-visible files.
    #[must_use]
    pub const fn read_only() -> Self {
        Self {
            allow_read: true,
            allow_write: false,
            allowed_prefixes: Vec::new(),
        }
    }

    /// Policy allowing read/write access to all provider-visible files.
    #[must_use]
    pub const fn read_write() -> Self {
        Self {
            allow_read: true,
            allow_write: true,
            allowed_prefixes: Vec::new(),
        }
    }

    fn permits(&self, path: &str, write: bool) -> bool {
        if write && !self.allow_write {
            return false;
        }
        if !write && !self.allow_read {
            return false;
        }
        self.allowed_prefixes.is_empty()
            || self
                .allowed_prefixes
                .iter()
                .any(|prefix| path == prefix || path.starts_with(&format!("{prefix}/")))
    }
}

/// Shell execution policy.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ShellPolicy {
    /// Whether shell execution is allowed.
    pub allow_execute: bool,
    /// Allowed program names. Empty means any program accepted by the provider.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_programs: Vec<String>,
}

impl ShellPolicy {
    /// Policy allowing all provider-visible commands.
    #[must_use]
    pub const fn allow_all() -> Self {
        Self {
            allow_execute: true,
            allowed_programs: Vec::new(),
        }
    }

    fn permits(&self, command: &str) -> bool {
        if !self.allow_execute {
            return false;
        }
        self.allowed_programs.is_empty()
            || command.split_whitespace().next().is_some_and(|program| {
                self.allowed_programs
                    .iter()
                    .any(|allowed| allowed == program)
            })
    }
}

/// Environment provider policy bundle.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct EnvironmentPolicy {
    /// Filesystem policy.
    pub files: FilePolicy,
    /// Shell policy.
    pub shell: ShellPolicy,
}

/// Glob options for provider-backed filesystem searches.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileGlobOptions {
    /// Include hidden dot paths.
    pub include_hidden: bool,
    /// Include provider-ignored files where the provider supports ignore files.
    pub include_ignored: bool,
    /// Maximum number of results to return. Zero means no explicit limit.
    pub max_results: usize,
}

impl Default for FileGlobOptions {
    fn default() -> Self {
        Self {
            include_hidden: false,
            include_ignored: false,
            max_results: 500,
        }
    }
}

/// Grep options for provider-backed text searches.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileGrepOptions {
    /// Ripgrep-style glob include filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include: Option<String>,
    /// Context lines before and after each match.
    pub context_lines: usize,
    /// Maximum total matches. Zero means no explicit limit.
    pub max_results: usize,
    /// Maximum matches per file. Zero means no explicit per-file limit.
    pub max_matches_per_file: usize,
    /// Maximum files to search. Zero means no explicit file limit.
    pub max_files: usize,
    /// Include hidden dot paths.
    pub include_hidden: bool,
    /// Include provider-ignored files where the provider supports ignore files.
    pub include_ignored: bool,
}

impl Default for FileGrepOptions {
    fn default() -> Self {
        Self {
            include: Some("**/*".to_string()),
            context_lines: 0,
            max_results: 100,
            max_matches_per_file: 20,
            max_files: 50,
            include_hidden: false,
            include_ignored: false,
        }
    }
}

/// Glob result entry.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileGlobMatch {
    /// Provider-scoped path.
    pub path: String,
}

/// Grep result entry.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileGrepMatch {
    /// Provider-scoped path.
    pub path: String,
    /// One-based line number.
    pub line_number: usize,
    /// Matching line without a trailing newline.
    pub matching_line: String,
    /// Context block containing the matching line.
    pub context: String,
    /// One-based line number where the context block starts.
    pub context_start_line: usize,
}

/// Provider-scoped file metadata.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileStat {
    /// File size in bytes. Directories report zero.
    pub size: u64,
    /// Whether the path resolves to a regular file.
    pub is_file: bool,
    /// Whether the path resolves to a directory.
    pub is_dir: bool,
    /// Modified timestamp in Unix seconds when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified_unix_seconds: Option<u64>,
}

/// Stable resource reference returned by environment providers.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResourceRef {
    /// Provider-specific resource identifier.
    pub id: String,
    /// Resource URI.
    pub uri: String,
    /// Resource metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

/// Shell execution request.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ShellCommand {
    /// Shell command string.
    pub command: String,
    /// Optional timeout in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    /// Optional provider-scoped working directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Environment variables to set for the command.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub environment: BTreeMap<String, String>,
}

/// Shell review workspace context exposed by environment providers.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ShellReviewEnvironmentContext {
    /// Default shell working directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_cwd: Option<String>,
    /// Provider-visible allowed paths.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_paths: Vec<String>,
    /// Shell platform.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell_platform: Option<String>,
    /// Shell executable when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell_executable: Option<String>,
}

/// Shell execution output.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ShellOutput {
    /// Process exit status code when available.
    pub status: i32,
    /// Captured stdout.
    pub stdout: String,
    /// Captured stderr.
    pub stderr: String,
    /// Execution metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

/// Serializable environment state snapshot.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct EnvironmentState {
    /// Provider identifier.
    pub provider_id: String,
    /// Logical files for virtual or resumable providers.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub files: BTreeMap<String, String>,
    /// Resource references.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resources: Vec<ResourceRef>,
    /// Provider metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

impl EnvironmentState {
    /// Convert the snapshot into JSON for `AgentContext` state domains.
    #[must_use]
    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or_else(|_| serde_json::json!({}))
    }
}

/// Background shell process status.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ShellProcessStatus {
    /// Process is still running.
    #[default]
    Running,
    /// Process completed.
    Completed,
    /// Process failed.
    Failed,
    /// Process was killed.
    Killed,
}

/// Durable shell process snapshot.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ShellProcessSnapshot {
    /// Stable process id.
    pub process_id: String,
    /// Original command.
    pub command: String,
    /// Process status.
    pub status: ShellProcessStatus,
    /// Buffered stdout since the last cursor observed by the provider.
    pub stdout: String,
    /// Buffered stderr since the last cursor observed by the provider.
    pub stderr: String,
    /// Exit status when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub return_code: Option<i32>,
    /// Provider metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

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

    /// Render model-facing context instructions for this environment.
    async fn get_context_instructions(&self) -> EnvironmentResult<Option<String>> {
        Ok(None)
    }

    /// Export provider state for resume.
    async fn export_state(&self) -> EnvironmentResult<EnvironmentState>;
}

/// Deterministic in-memory environment provider for tests and previews.
#[derive(Clone, Debug)]
pub struct VirtualEnvironmentProvider {
    id: String,
    policy: EnvironmentPolicy,
    tmp_namespace: Option<String>,
    files: Arc<Mutex<BTreeMap<String, String>>>,
    binary_files: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
    directories: Arc<Mutex<BTreeSet<String>>>,
    shell_outputs: Arc<Mutex<BTreeMap<String, ShellOutput>>>,
    processes: Arc<Mutex<BTreeMap<String, ShellProcessSnapshot>>>,
}

impl VirtualEnvironmentProvider {
    /// Create a virtual provider.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            policy: EnvironmentPolicy {
                files: FilePolicy::read_write(),
                shell: ShellPolicy::allow_all(),
            },
            tmp_namespace: None,
            files: Arc::new(Mutex::new(BTreeMap::new())),
            binary_files: Arc::new(Mutex::new(BTreeMap::new())),
            directories: Arc::new(Mutex::new(BTreeSet::new())),
            shell_outputs: Arc::new(Mutex::new(BTreeMap::new())),
            processes: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    /// Set provider policy.
    #[must_use]
    pub fn with_policy(mut self, policy: EnvironmentPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Set a provider-scoped temporary file namespace.
    ///
    /// Namespaces isolate tool-generated large output files under a stable
    /// subdirectory of the provider temporary root.
    #[must_use]
    pub fn with_tmp_namespace(mut self, namespace: impl AsRef<str>) -> Self {
        self.tmp_namespace = normalize_tmp_namespace(namespace.as_ref()).ok();
        self
    }

    /// Add a virtual UTF-8 text file.
    #[must_use]
    pub fn with_file(self, path: impl Into<String>, content: impl Into<String>) -> Self {
        let path = path.into();
        if let Ok(mut files) = self.files.lock() {
            files.insert(path.clone(), content.into());
        }
        if let Ok(mut binary_files) = self.binary_files.lock() {
            binary_files.remove(&path);
        }
        self
    }

    /// Add a virtual binary file.
    #[must_use]
    pub fn with_bytes(self, path: impl Into<String>, content: impl Into<Vec<u8>>) -> Self {
        let path = path.into();
        if let Ok(mut binary_files) = self.binary_files.lock() {
            binary_files.insert(path.clone(), content.into());
        }
        if let Ok(mut files) = self.files.lock() {
            files.remove(&path);
        }
        self
    }

    /// Add deterministic shell output.
    #[must_use]
    pub fn with_shell_output(self, command: impl Into<String>, output: ShellOutput) -> Self {
        if let Ok(mut shell_outputs) = self.shell_outputs.lock() {
            shell_outputs.insert(command.into(), output);
        }
        self
    }

    /// Add a deterministic background process snapshot.
    #[must_use]
    pub fn with_process(self, snapshot: ShellProcessSnapshot) -> Self {
        if let Ok(mut processes) = self.processes.lock() {
            processes.insert(snapshot.process_id.clone(), snapshot);
        }
        self
    }

    fn check_file(&self, path: &str, write: bool) -> EnvironmentResult<()> {
        if is_tmp_path(path) || self.policy.files.permits(path, write) {
            Ok(())
        } else {
            Err(EnvironmentError::AccessDenied(path.to_string()))
        }
    }

    fn all_file_keys(&self) -> EnvironmentResult<Vec<String>> {
        let mut keys = BTreeSet::new();
        keys.extend(
            self.files
                .lock()
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?
                .keys()
                .cloned(),
        );
        keys.extend(
            self.binary_files
                .lock()
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?
                .keys()
                .cloned(),
        );
        Ok(keys.into_iter().collect())
    }

    fn all_dir_keys(&self) -> EnvironmentResult<Vec<String>> {
        Ok(self
            .directories
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .iter()
            .cloned()
            .collect())
    }

    fn insert_directory_ancestors(&self, path: &str) -> EnvironmentResult<()> {
        {
            let mut directories = self
                .directories
                .lock()
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
            for ancestor in logical_ancestors(path) {
                directories.insert(ancestor);
            }
        }
        Ok(())
    }

    fn tmp_file_path(&self, filename: &str) -> EnvironmentResult<String> {
        let filename = normalize_tmp_filename(filename)?;
        let relative = self.tmp_namespace.as_deref().map_or_else(
            || filename.clone(),
            |namespace| join_logical_path(namespace, &filename),
        );
        Ok(join_logical_path(DEFAULT_TMP_DIR, &relative))
    }

    fn path_exists_unchecked(&self, path: &str) -> EnvironmentResult<bool> {
        if self
            .files
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .contains_key(path)
            || self
                .binary_files
                .lock()
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?
                .contains_key(path)
            || self
                .directories
                .lock()
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?
                .contains(path)
        {
            return Ok(true);
        }
        let prefix = if path.is_empty() {
            String::new()
        } else {
            format!("{}/", path.trim_end_matches('/'))
        };
        Ok(self
            .all_file_keys()?
            .iter()
            .any(|entry| entry.starts_with(&prefix))
            || self
                .all_dir_keys()?
                .iter()
                .any(|entry| entry.starts_with(&prefix)))
    }

    fn ensure_virtual_destination(
        &self,
        src: &str,
        dst: &str,
        overwrite: bool,
    ) -> EnvironmentResult<()> {
        if src == dst {
            return Err(EnvironmentError::InvalidRequest(
                "source and destination must differ".to_string(),
            ));
        }
        if self.path_exists_unchecked(dst)? && !overwrite {
            return Err(EnvironmentError::InvalidRequest(format!(
                "destination already exists: {dst}"
            )));
        }
        Ok(())
    }
}

#[async_trait]
impl ProcessShellProvider for VirtualEnvironmentProvider {
    async fn start_process(
        &self,
        command: ShellCommand,
    ) -> EnvironmentResult<ShellProcessSnapshot> {
        if !self.policy.shell.permits(&command.command) {
            return Err(EnvironmentError::AccessDenied(command.command));
        }
        let process_id = format!(
            "process_{}",
            self.processes
                .lock()
                .map_or(0, |processes| processes.len() + 1)
        );
        let mut metadata = Metadata::default();
        if let Some(timeout_seconds) = command.timeout_seconds {
            metadata.insert(
                "timeout_seconds".to_string(),
                serde_json::json!(timeout_seconds),
            );
        }
        if let Some(cwd) = &command.cwd {
            metadata.insert("cwd".to_string(), serde_json::json!(cwd));
        }
        if !command.environment.is_empty() {
            metadata.insert(
                "environment".to_string(),
                serde_json::json!(command.environment),
            );
        }
        let snapshot = ShellProcessSnapshot {
            process_id: process_id.clone(),
            command: command.command,
            status: ShellProcessStatus::Running,
            stdout: String::new(),
            stderr: String::new(),
            return_code: None,
            metadata,
        };
        self.processes
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .insert(process_id, snapshot.clone());
        Ok(snapshot)
    }

    async fn wait_process(
        &self,
        process_id: &str,
        _timeout_seconds: u64,
    ) -> EnvironmentResult<ShellProcessSnapshot> {
        self.processes
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .get(process_id)
            .cloned()
            .ok_or_else(|| EnvironmentError::NotFound(process_id.to_string()))
    }

    async fn list_processes(&self) -> EnvironmentResult<Vec<ShellProcessSnapshot>> {
        Ok(self
            .processes
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .values()
            .cloned()
            .collect())
    }

    async fn input_process(
        &self,
        process_id: &str,
        text: &str,
        close_stdin: bool,
    ) -> EnvironmentResult<ShellProcessSnapshot> {
        let mut processes = self
            .processes
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
        let snapshot = processes
            .get_mut(process_id)
            .ok_or_else(|| EnvironmentError::NotFound(process_id.to_string()))?;
        snapshot
            .metadata
            .insert("last_input".to_string(), serde_json::json!(text));
        snapshot
            .metadata
            .insert("close_stdin".to_string(), serde_json::json!(close_stdin));
        let snapshot = snapshot.clone();
        drop(processes);
        Ok(snapshot)
    }

    async fn signal_process(
        &self,
        process_id: &str,
        signal: i32,
    ) -> EnvironmentResult<ShellProcessSnapshot> {
        let mut processes = self
            .processes
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
        let snapshot = processes
            .get_mut(process_id)
            .ok_or_else(|| EnvironmentError::NotFound(process_id.to_string()))?;
        snapshot
            .metadata
            .insert("last_signal".to_string(), serde_json::json!(signal));
        let snapshot = snapshot.clone();
        drop(processes);
        Ok(snapshot)
    }

    async fn kill_process(&self, process_id: &str) -> EnvironmentResult<ShellProcessSnapshot> {
        let mut processes = self
            .processes
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
        let snapshot = processes
            .get_mut(process_id)
            .ok_or_else(|| EnvironmentError::NotFound(process_id.to_string()))?;
        snapshot.status = ShellProcessStatus::Killed;
        let snapshot = snapshot.clone();
        drop(processes);
        Ok(snapshot)
    }
}

#[async_trait]
impl EnvironmentProvider for VirtualEnvironmentProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn process_shell_provider(self: Arc<Self>) -> Option<DynProcessShellProvider> {
        Some(self)
    }

    fn shell_review_context(&self) -> ShellReviewEnvironmentContext {
        ShellReviewEnvironmentContext {
            default_cwd: Some(".".to_string()),
            allowed_paths: vec![".".to_string()],
            shell_platform: Some("virtual".to_string()),
            shell_executable: None,
        }
    }

    async fn read_text(&self, path: &str) -> EnvironmentResult<String> {
        self.check_file(path, false)?;
        let text_content = self
            .files
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .get(path)
            .cloned();
        if let Some(content) = text_content {
            return Ok(content);
        }
        let bytes = self
            .binary_files
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .get(path)
            .cloned()
            .ok_or_else(|| EnvironmentError::NotFound(path.to_string()))?;
        String::from_utf8(bytes).map_err(|error| EnvironmentError::Provider(error.to_string()))
    }

    async fn read_bytes(
        &self,
        path: &str,
        offset: usize,
        length: Option<usize>,
    ) -> EnvironmentResult<Vec<u8>> {
        self.check_file(path, false)?;
        let text_content = self
            .files
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .get(path)
            .cloned();
        let bytes = if let Some(content) = text_content {
            content.into_bytes()
        } else {
            self.binary_files
                .lock()
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?
                .get(path)
                .cloned()
                .ok_or_else(|| EnvironmentError::NotFound(path.to_string()))?
        };
        if offset >= bytes.len() {
            return Ok(Vec::new());
        }
        let end = length.map_or(bytes.len(), |length| {
            offset.saturating_add(length).min(bytes.len())
        });
        Ok(bytes[offset..end].to_vec())
    }

    async fn write_text(&self, path: &str, content: &str) -> EnvironmentResult<()> {
        self.check_file(path, true)?;
        self.files
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .insert(path.to_string(), content.to_string());
        self.binary_files
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .remove(path);
        Ok(())
    }

    async fn create_dir(&self, path: &str, parents: bool) -> EnvironmentResult<()> {
        let normalized = normalize_requested_path(path)?;
        self.check_file(&normalized, true)?;
        if normalized.is_empty() || normalized == "." {
            return Ok(());
        }
        if self
            .files
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .contains_key(&normalized)
            || self
                .binary_files
                .lock()
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?
                .contains_key(&normalized)
        {
            return Err(EnvironmentError::InvalidRequest(format!(
                "path already exists as a file: {normalized}"
            )));
        }
        if parents {
            self.insert_directory_ancestors(&normalized)?;
        } else if let Some(parent) = parent_path(&normalized) {
            if !parent.is_empty() && !self.path_exists_unchecked(&parent)? {
                return Err(EnvironmentError::NotFound(parent));
            }
        }
        self.directories
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .insert(normalized);
        Ok(())
    }

    async fn delete_path(&self, path: &str, recursive: bool) -> EnvironmentResult<()> {
        let normalized = normalize_requested_path(path)?;
        self.check_file(&normalized, true)?;
        let removed_file = self
            .files
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .remove(&normalized)
            .is_some()
            || self
                .binary_files
                .lock()
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?
                .remove(&normalized)
                .is_some();
        if removed_file {
            return Ok(());
        }
        let prefix = format!("{}/", normalized.trim_end_matches('/'));
        let file_children = self
            .all_file_keys()?
            .into_iter()
            .filter(|entry| entry.starts_with(&prefix))
            .collect::<Vec<_>>();
        let dir_children = self
            .all_dir_keys()?
            .into_iter()
            .filter(|entry| entry.starts_with(&prefix))
            .collect::<Vec<_>>();
        let explicit_dir = self
            .directories
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .contains(&normalized);
        if !explicit_dir && file_children.is_empty() && dir_children.is_empty() {
            return Err(EnvironmentError::NotFound(path.to_string()));
        }
        if !recursive && (!file_children.is_empty() || !dir_children.is_empty()) {
            return Err(EnvironmentError::InvalidRequest(format!(
                "directory is not empty: {normalized}"
            )));
        }
        self.files
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .retain(|entry, _| entry != &normalized && !entry.starts_with(&prefix));
        self.binary_files
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .retain(|entry, _| entry != &normalized && !entry.starts_with(&prefix));
        self.directories
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .retain(|entry| entry != &normalized && !entry.starts_with(&prefix));
        Ok(())
    }

    async fn move_path(&self, src: &str, dst: &str, overwrite: bool) -> EnvironmentResult<()> {
        let src = normalize_requested_path(src)?;
        let dst = normalize_requested_path(dst)?;
        self.check_file(&src, true)?;
        self.check_file(&dst, true)?;
        self.ensure_virtual_destination(&src, &dst, overwrite)?;
        self.copy_path(&src, &dst, overwrite).await?;
        self.delete_path(&src, true).await
    }

    async fn copy_path(&self, src: &str, dst: &str, overwrite: bool) -> EnvironmentResult<()> {
        let src = normalize_requested_path(src)?;
        let dst = normalize_requested_path(dst)?;
        self.check_file(&src, false)?;
        self.check_file(&dst, true)?;
        self.ensure_virtual_destination(&src, &dst, overwrite)?;
        if overwrite && self.path_exists_unchecked(&dst)? {
            self.delete_path(&dst, true).await?;
        }

        let text_content = self
            .files
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .get(&src)
            .cloned();
        if let Some(content) = text_content {
            self.files
                .lock()
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?
                .insert(dst.clone(), content);
            self.binary_files
                .lock()
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?
                .remove(&dst);
            return Ok(());
        }
        let binary_content = self
            .binary_files
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .get(&src)
            .cloned();
        if let Some(content) = binary_content {
            self.binary_files
                .lock()
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?
                .insert(dst.clone(), content);
            self.files
                .lock()
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?
                .remove(&dst);
            return Ok(());
        }

        let prefix = format!("{}/", src.trim_end_matches('/'));
        let text_entries = self
            .files
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .iter()
            .filter(|(path, _)| path.starts_with(&prefix))
            .map(|(path, content)| (path.clone(), content.clone()))
            .collect::<Vec<_>>();
        let binary_entries = self
            .binary_files
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .iter()
            .filter(|(path, _)| path.starts_with(&prefix))
            .map(|(path, content)| (path.clone(), content.clone()))
            .collect::<Vec<_>>();
        let dir_entries = self
            .all_dir_keys()?
            .into_iter()
            .filter(|path| path == &src || path.starts_with(&prefix))
            .collect::<Vec<_>>();
        if text_entries.is_empty() && binary_entries.is_empty() && dir_entries.is_empty() {
            return Err(EnvironmentError::NotFound(src));
        }

        for dir in dir_entries {
            let target = replace_logical_prefix(&dir, &src, &dst);
            self.directories
                .lock()
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?
                .insert(target);
        }
        for (path, content) in text_entries {
            let target = replace_logical_prefix(&path, &src, &dst);
            self.files
                .lock()
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?
                .insert(target.clone(), content);
        }
        for (path, content) in binary_entries {
            let target = replace_logical_prefix(&path, &src, &dst);
            self.binary_files
                .lock()
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?
                .insert(target.clone(), content);
        }
        Ok(())
    }

    async fn write_tmp_file(&self, filename: &str, content: &[u8]) -> EnvironmentResult<String> {
        let normalized = self.tmp_file_path(filename)?;
        self.binary_files
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .insert(normalized.clone(), content.to_vec());
        self.files
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .remove(&normalized);
        Ok(normalized)
    }

    async fn stat(&self, path: &str) -> EnvironmentResult<FileStat> {
        self.check_file(path, false)?;
        let text_content = self
            .files
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .get(path)
            .cloned();
        if let Some(content) = text_content {
            return Ok(FileStat {
                size: content.len() as u64,
                is_file: true,
                is_dir: false,
                modified_unix_seconds: None,
            });
        }
        let binary_content = self
            .binary_files
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .get(path)
            .cloned();
        if let Some(content) = binary_content {
            return Ok(FileStat {
                size: content.len() as u64,
                is_file: true,
                is_dir: false,
                modified_unix_seconds: None,
            });
        }
        if self
            .directories
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .contains(path)
        {
            return Ok(FileStat {
                size: 0,
                is_file: false,
                is_dir: true,
                modified_unix_seconds: None,
            });
        }
        let prefix = if path.is_empty() {
            String::new()
        } else {
            format!("{}/", path.trim_end_matches('/'))
        };
        if self
            .all_file_keys()?
            .iter()
            .any(|entry| entry.starts_with(&prefix))
            || self
                .all_dir_keys()?
                .iter()
                .any(|entry| entry.starts_with(&prefix))
        {
            return Ok(FileStat {
                size: 0,
                is_file: false,
                is_dir: true,
                modified_unix_seconds: None,
            });
        }
        Err(EnvironmentError::NotFound(path.to_string()))
    }

    async fn list(&self, path: &str) -> EnvironmentResult<Vec<String>> {
        self.check_file(path, false)?;
        let prefix = if path.is_empty() {
            String::new()
        } else {
            format!("{}/", path.trim_end_matches('/'))
        };
        let mut entries = BTreeSet::new();
        entries.extend(
            self.all_file_keys()?
                .into_iter()
                .filter(|entry| entry.starts_with(&prefix)),
        );
        entries.extend(
            self.all_dir_keys()?
                .into_iter()
                .filter(|entry| entry.starts_with(&prefix)),
        );
        Ok(entries.into_iter().collect())
    }

    async fn glob(
        &self,
        path: &str,
        pattern: &str,
        options: FileGlobOptions,
    ) -> EnvironmentResult<Vec<FileGlobMatch>> {
        self.check_file(path, false)?;
        let prefix = path.trim_matches('/');
        let path_glob = PathGlob::new(pattern)?;
        let mut glob_matches = Vec::new();
        for entry in self.all_file_keys()? {
            if path_contains(prefix, &entry)
                && include_path(&entry, options.include_hidden)
                && path_glob.is_match(strip_path_prefix(prefix, &entry))
            {
                glob_matches.push(FileGlobMatch { path: entry });
                if options.max_results > 0 && glob_matches.len() >= options.max_results {
                    break;
                }
            }
        }
        Ok(glob_matches)
    }

    async fn run_shell(&self, command: ShellCommand) -> EnvironmentResult<ShellOutput> {
        if !self.policy.shell.permits(&command.command) {
            return Err(EnvironmentError::AccessDenied(command.command));
        }
        self.shell_outputs
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .get(&command.command)
            .cloned()
            .ok_or(EnvironmentError::NotFound(command.command))
    }

    async fn get_context_instructions(&self) -> EnvironmentResult<Option<String>> {
        let mut files = self
            .files
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .clone();
        for path in self
            .binary_files
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .keys()
        {
            files.entry(path.clone()).or_default();
        }
        let tree = generate_virtual_file_tree(&files, ".", DEFAULT_INSTRUCTIONS_MAX_DEPTH);
        Ok(Some(render_environment_context(
            self.id(),
            ".",
            &[FileTreeBlock {
                path: ".".to_string(),
                tree,
            }],
            self.policy.shell.allow_execute,
            None,
        )))
    }

    async fn export_state(&self) -> EnvironmentResult<EnvironmentState> {
        Ok(EnvironmentState {
            provider_id: self.id.clone(),
            files: self
                .files
                .lock()
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?
                .clone(),
            resources: Vec::new(),
            metadata: Metadata::default(),
        })
    }
}

/// Local provider skeleton with policy-aware filesystem access.
#[derive(Clone, Debug)]
pub struct LocalEnvironmentProvider {
    id: String,
    root: PathBuf,
    allowed_paths: Vec<PathBuf>,
    tmp_dir: Option<Arc<tempfile::TempDir>>,
    tmp_namespace: Option<String>,
    policy: EnvironmentPolicy,
    processes: Arc<Mutex<BTreeMap<String, LocalShellProcess>>>,
}

impl LocalEnvironmentProvider {
    /// Create a local provider rooted at a directory.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = normalize_local_config_path(root.into());
        let tmp_dir = create_local_tmp_dir(None).map(Arc::new);
        let mut allowed_paths = vec![root.clone()];
        if let Some(tmp_path) = tmp_dir.as_ref().map(|tmp_dir| tmp_dir.path()) {
            push_unique_path(
                &mut allowed_paths,
                normalize_local_config_path(tmp_path.to_path_buf()),
            );
        }
        Self {
            id: "local".to_string(),
            allowed_paths,
            tmp_dir,
            tmp_namespace: None,
            root,
            policy: EnvironmentPolicy {
                files: FilePolicy::read_only(),
                shell: ShellPolicy::default(),
            },
            processes: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    /// Set provider id.
    #[must_use]
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    /// Set policy.
    #[must_use]
    pub fn with_policy(mut self, policy: EnvironmentPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Set absolute filesystem roots that this provider may access.
    ///
    /// The provider root remains the default directory for relative paths and is
    /// always included even when omitted from `paths`.
    #[must_use]
    pub fn with_allowed_paths<I, P>(mut self, paths: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: Into<PathBuf>,
    {
        let allowed_paths = paths
            .into_iter()
            .map(|path| normalize_local_config_path(path.into()))
            .collect::<Vec<_>>();
        self.rebuild_allowed_paths_with_managed_roots(allowed_paths);
        self
    }

    /// Create provider-managed temporary files under a specific base directory.
    ///
    /// The created session directory is added to the allowed path set and is
    /// cleaned up when the provider and its clones are dropped. If creation
    /// fails, the existing temporary directory configuration is left unchanged.
    #[must_use]
    pub fn with_tmp_base_dir(mut self, base_dir: impl Into<PathBuf>) -> Self {
        let old_tmp_dir = self
            .tmp_dir_path()
            .map(|path| normalize_local_config_path(path.to_path_buf()));
        let base_dir = normalize_local_config_path(base_dir.into());
        if let Some(tmp_dir) = create_local_tmp_dir(Some(&base_dir)) {
            self.tmp_dir = Some(Arc::new(tmp_dir));
            let allowed_paths = self
                .allowed_paths
                .iter()
                .filter(|path| old_tmp_dir.as_ref() != Some(path))
                .cloned()
                .collect::<Vec<_>>();
            self.rebuild_allowed_paths_with_managed_roots(allowed_paths);
        }
        self
    }

    /// Set a provider-scoped temporary file namespace.
    ///
    /// Namespaces isolate tool-generated large output files under a stable
    /// subdirectory of the provider temporary root.
    #[must_use]
    pub fn with_tmp_namespace(mut self, namespace: impl AsRef<str>) -> Self {
        self.tmp_namespace = normalize_tmp_namespace(namespace.as_ref()).ok();
        self
    }

    /// Return configured local filesystem roots.
    #[must_use]
    pub fn allowed_paths(&self) -> &[PathBuf] {
        &self.allowed_paths
    }

    /// Return this provider's managed temporary directory when available.
    #[must_use]
    pub fn tmp_dir_path(&self) -> Option<&Path> {
        self.tmp_dir.as_ref().map(|tmp_dir| tmp_dir.path())
    }

    fn resolve(&self, path: &str, write: bool) -> EnvironmentResult<PathBuf> {
        let (visible_path, filesystem_path) = self.resolve_with_visible_path(path)?;
        if !self.path_is_managed_tmp(&filesystem_path)
            && !self.policy.files.permits(&visible_path, write)
        {
            return Err(EnvironmentError::AccessDenied(path.to_string()));
        }
        Ok(filesystem_path)
    }

    fn resolve_with_visible_path(&self, path: &str) -> EnvironmentResult<(String, PathBuf)> {
        let requested = Path::new(path);
        if requested.is_absolute() {
            let filesystem_path = normalize_absolute_request_path(requested)?;
            if !self.path_is_allowed(&filesystem_path) {
                return Err(EnvironmentError::AccessDenied(path.to_string()));
            }
            let visible_path = self.logical_path(&filesystem_path)?;
            return Ok((visible_path, filesystem_path));
        }

        let mut logical_path = normalize_requested_path(path)?;
        if logical_path == "." {
            logical_path.clear();
        }
        if let Some(tmp_path) = self.managed_tmp_path(&logical_path)? {
            return Ok((logical_path, tmp_path));
        }
        Ok((logical_path.clone(), self.root.join(&logical_path)))
    }

    fn path_is_allowed(&self, path: &Path) -> bool {
        self.allowed_paths
            .iter()
            .any(|allowed_path| path == allowed_path || path.starts_with(allowed_path))
    }

    fn path_is_allowed_root(&self, path: &Path) -> bool {
        self.allowed_paths
            .iter()
            .any(|allowed_path| path == allowed_path)
    }

    fn logical_path(&self, path: &Path) -> EnvironmentResult<String> {
        let path = normalize_local_config_path(path.to_path_buf());
        if self.path_is_managed_tmp(&path) {
            return Ok(display_local_path(&path));
        }
        if let Ok(relative) = path.strip_prefix(&self.root) {
            return Ok(normalize_path(relative));
        }
        if self.path_is_allowed(&path) {
            return Ok(display_local_path(&path));
        }
        Err(EnvironmentError::AccessDenied(path.display().to_string()))
    }

    fn visible_root_for_allowed_path(&self, path: &Path) -> String {
        let path = normalize_local_config_path(path.to_path_buf());
        path.strip_prefix(&self.root)
            .map_or_else(|_| display_local_path(&path), normalize_path)
    }

    fn resolve_shell_cwd(&self, cwd: Option<&str>) -> EnvironmentResult<PathBuf> {
        let cwd = match cwd {
            Some(cwd) => self.resolve(cwd, false)?,
            None => self.root.clone(),
        };
        if !cwd.is_dir() {
            return Err(EnvironmentError::InvalidRequest(format!(
                "shell cwd is not a directory: {}",
                cwd.display()
            )));
        }
        Ok(cwd)
    }

    fn rebuild_allowed_paths_with_managed_roots(&mut self, paths: Vec<PathBuf>) {
        let mut allowed_paths = Vec::new();
        for path in paths {
            push_unique_path(&mut allowed_paths, normalize_local_config_path(path));
        }
        push_unique_path(&mut allowed_paths, self.root.clone());
        if let Some(tmp_dir) = self.tmp_dir_path() {
            push_unique_path(
                &mut allowed_paths,
                normalize_local_config_path(tmp_dir.to_path_buf()),
            );
        }
        self.allowed_paths = allowed_paths;
    }

    fn tmp_file_relative_path(&self, filename: &str) -> EnvironmentResult<String> {
        let filename = normalize_tmp_filename(filename)?;
        Ok(self.tmp_namespace.as_deref().map_or_else(
            || filename.clone(),
            |namespace| join_logical_path(namespace, &filename),
        ))
    }

    fn managed_tmp_path(&self, logical_path: &str) -> EnvironmentResult<Option<PathBuf>> {
        if !is_tmp_path(logical_path) {
            return Ok(None);
        }
        let Some(tmp_dir) = self.tmp_dir_path() else {
            return Ok(None);
        };
        let normalized = normalize_str_path(logical_path);
        let relative = normalized
            .strip_prefix(DEFAULT_TMP_DIR)
            .and_then(|suffix| suffix.strip_prefix('/'))
            .unwrap_or_default();
        if relative.is_empty() {
            return Ok(Some(tmp_dir.to_path_buf()));
        }
        Ok(Some(tmp_dir.join(normalize_requested_path(relative)?)))
    }

    fn path_is_managed_tmp(&self, path: &Path) -> bool {
        let path = normalize_local_config_path(path.to_path_buf());
        self.tmp_dir_path().is_some_and(|tmp_dir| {
            let tmp_dir = normalize_local_config_path(tmp_dir.to_path_buf());
            path == tmp_dir || path.starts_with(tmp_dir)
        })
    }
}

#[derive(Debug)]
struct LocalShellProcess {
    command: String,
    child: Child,
    stdout_handle: Option<thread::JoinHandle<io::Result<String>>>,
    stderr_handle: Option<thread::JoinHandle<io::Result<String>>>,
    metadata: Metadata,
    completed: Option<ShellProcessSnapshot>,
}

#[async_trait]
#[allow(clippy::significant_drop_tightening)]
impl ProcessShellProvider for LocalEnvironmentProvider {
    async fn start_process(
        &self,
        command: ShellCommand,
    ) -> EnvironmentResult<ShellProcessSnapshot> {
        if !self.policy.shell.permits(&command.command) {
            return Err(EnvironmentError::AccessDenied(command.command));
        }
        let cwd = self.resolve_shell_cwd(command.cwd.as_deref())?;
        let mut child = Command::new("/bin/sh")
            .arg("-lc")
            .arg(&command.command)
            .current_dir(cwd)
            .envs(&command.environment)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
        let mut stdout_reader = child.stdout.take();
        let mut stderr_reader = child.stderr.take();
        let stdout_handle = thread::spawn(move || read_child_pipe(stdout_reader.take()));
        let stderr_handle = thread::spawn(move || read_child_pipe(stderr_reader.take()));
        let process_id = format!("process_{}", child.id());
        let metadata = shell_process_metadata(&command);
        let snapshot = ShellProcessSnapshot {
            process_id: process_id.clone(),
            command: command.command.clone(),
            status: ShellProcessStatus::Running,
            stdout: String::new(),
            stderr: String::new(),
            return_code: None,
            metadata: metadata.clone(),
        };
        self.processes
            .lock()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
            .insert(
                process_id,
                LocalShellProcess {
                    command: command.command,
                    child,
                    stdout_handle: Some(stdout_handle),
                    stderr_handle: Some(stderr_handle),
                    metadata,
                    completed: None,
                },
            );
        Ok(snapshot)
    }

    async fn wait_process(
        &self,
        process_id: &str,
        timeout_seconds: u64,
    ) -> EnvironmentResult<ShellProcessSnapshot> {
        let deadline = Instant::now() + Duration::from_secs(timeout_seconds);
        loop {
            let snapshot = {
                let mut processes = self
                    .processes
                    .lock()
                    .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
                let process = processes
                    .get_mut(process_id)
                    .ok_or_else(|| EnvironmentError::NotFound(process_id.to_string()))?;
                refresh_local_shell_process(process_id, process, false)?
            };
            if snapshot.status != ShellProcessStatus::Running || timeout_seconds == 0 {
                return Ok(snapshot);
            }
            if Instant::now() >= deadline {
                return Ok(snapshot);
            }
            thread::sleep(Duration::from_millis(25));
        }
    }

    async fn list_processes(&self) -> EnvironmentResult<Vec<ShellProcessSnapshot>> {
        let snapshots = {
            let mut processes = self
                .processes
                .lock()
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
            let mut snapshots = Vec::new();
            for (process_id, process) in processes.iter_mut() {
                snapshots.push(refresh_local_shell_process(process_id, process, false)?);
            }
            snapshots
        };
        Ok(snapshots)
    }

    async fn input_process(
        &self,
        process_id: &str,
        text: &str,
        close_stdin: bool,
    ) -> EnvironmentResult<ShellProcessSnapshot> {
        let snapshot = {
            let mut processes = self
                .processes
                .lock()
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
            let process = processes
                .get_mut(process_id)
                .ok_or_else(|| EnvironmentError::NotFound(process_id.to_string()))?;
            if let Some(stdin) = process.child.stdin.as_mut() {
                use std::io::Write as _;
                stdin
                    .write_all(text.as_bytes())
                    .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
                stdin
                    .write_all(b"\n")
                    .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
                if close_stdin {
                    process.child.stdin.take();
                }
            } else {
                return Err(EnvironmentError::InvalidRequest(format!(
                    "stdin is closed for process: {process_id}"
                )));
            }
            refresh_local_shell_process(process_id, process, false)?
        };
        Ok(snapshot)
    }

    async fn signal_process(
        &self,
        process_id: &str,
        signal: i32,
    ) -> EnvironmentResult<ShellProcessSnapshot> {
        let snapshot = {
            let mut processes = self
                .processes
                .lock()
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
            let process = processes
                .get_mut(process_id)
                .ok_or_else(|| EnvironmentError::NotFound(process_id.to_string()))?;
            #[cfg(unix)]
            {
                let pid = process.child.id().to_string();
                let status = Command::new("kill")
                    .arg(format!("-{signal}"))
                    .arg(pid)
                    .status()
                    .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
                if !status.success() {
                    return Err(EnvironmentError::Provider(format!(
                        "failed to signal process {process_id} with signal {signal}"
                    )));
                }
            }
            #[cfg(not(unix))]
            {
                return Err(EnvironmentError::InvalidRequest(
                    "shell_signal is only supported on Unix local providers".to_string(),
                ));
            }
            refresh_local_shell_process(process_id, process, false)?
        };
        Ok(snapshot)
    }

    async fn kill_process(&self, process_id: &str) -> EnvironmentResult<ShellProcessSnapshot> {
        let snapshot = {
            let mut processes = self
                .processes
                .lock()
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
            let process = processes
                .get_mut(process_id)
                .ok_or_else(|| EnvironmentError::NotFound(process_id.to_string()))?;
            refresh_local_shell_process(process_id, process, true)?
        };
        Ok(snapshot)
    }
}

#[async_trait]
impl EnvironmentProvider for LocalEnvironmentProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn process_shell_provider(self: Arc<Self>) -> Option<DynProcessShellProvider> {
        Some(self)
    }

    fn shell_review_context(&self) -> ShellReviewEnvironmentContext {
        ShellReviewEnvironmentContext {
            default_cwd: Some(display_local_path(&self.root)),
            allowed_paths: self
                .allowed_paths
                .iter()
                .map(|path| display_local_path(path))
                .collect(),
            shell_platform: Some(std::env::consts::OS.to_string()),
            shell_executable: std::env::var("SHELL").ok(),
        }
    }

    fn path_match_candidates(&self, path: &str) -> Vec<String> {
        let mut candidates = path_match_candidates(path);
        if let Ok((visible_path, filesystem_path)) = self.resolve_with_visible_path(path) {
            push_unique_candidate(&mut candidates, visible_path.clone());
            push_unique_candidate(&mut candidates, normalize_match_path(&visible_path));
            push_unique_candidate(&mut candidates, display_local_path(&filesystem_path));
        }
        candidates
    }

    async fn read_text(&self, path: &str) -> EnvironmentResult<String> {
        let path = self.resolve(path, false)?;
        std::fs::read_to_string(&path)
            .map_err(|error| EnvironmentError::Provider(error.to_string()))
    }

    async fn read_bytes(
        &self,
        path: &str,
        offset: usize,
        length: Option<usize>,
    ) -> EnvironmentResult<Vec<u8>> {
        let path = self.resolve(path, false)?;
        let bytes =
            std::fs::read(&path).map_err(|error| EnvironmentError::Provider(error.to_string()))?;
        if offset >= bytes.len() {
            return Ok(Vec::new());
        }
        let end = length.map_or(bytes.len(), |length| {
            offset.saturating_add(length).min(bytes.len())
        });
        Ok(bytes[offset..end].to_vec())
    }

    async fn write_text(&self, path: &str, content: &str) -> EnvironmentResult<()> {
        let path = self.resolve(path, true)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
        }
        std::fs::write(&path, content).map_err(|error| map_io_error(&path, &error))
    }

    async fn create_dir(&self, path: &str, parents: bool) -> EnvironmentResult<()> {
        let path = self.resolve(path, true)?;
        if self.path_is_allowed_root(&path) && path.exists() {
            return Ok(());
        }
        let result = if parents {
            std::fs::create_dir_all(&path)
        } else {
            std::fs::create_dir(&path)
        };
        result.map_err(|error| map_io_error(&path, &error))
    }

    async fn delete_path(&self, path: &str, recursive: bool) -> EnvironmentResult<()> {
        let path = self.resolve(path, true)?;
        if self.path_is_allowed_root(&path) {
            return Err(EnvironmentError::InvalidRequest(
                "refusing to delete an allowed environment root".to_string(),
            ));
        }
        let metadata = std::fs::metadata(&path).map_err(|error| map_io_error(&path, &error))?;
        if metadata.is_dir() {
            if recursive {
                std::fs::remove_dir_all(&path)
            } else {
                std::fs::remove_dir(&path)
            }
        } else {
            std::fs::remove_file(&path)
        }
        .map_err(|error| map_io_error(&path, &error))
    }

    async fn move_path(&self, src: &str, dst: &str, overwrite: bool) -> EnvironmentResult<()> {
        let src = self.resolve(src, true)?;
        let dst = self.resolve(dst, true)?;
        if self.path_is_allowed_root(&src) {
            return Err(EnvironmentError::InvalidRequest(
                "refusing to move an allowed environment root".to_string(),
            ));
        }
        if self.path_is_allowed_root(&dst) {
            return Err(EnvironmentError::InvalidRequest(
                "destination must not be an allowed environment root".to_string(),
            ));
        }
        if src == dst {
            return Err(EnvironmentError::InvalidRequest(
                "source and destination must differ".to_string(),
            ));
        }
        prepare_local_destination(&dst, overwrite)?;
        std::fs::rename(&src, &dst).map_err(|error| map_io_error(&src, &error))
    }

    async fn copy_path(&self, src: &str, dst: &str, overwrite: bool) -> EnvironmentResult<()> {
        let src = self.resolve(src, false)?;
        let dst = self.resolve(dst, true)?;
        if self.path_is_allowed_root(&src) {
            return Err(EnvironmentError::InvalidRequest(
                "refusing to copy an allowed environment root".to_string(),
            ));
        }
        if self.path_is_allowed_root(&dst) {
            return Err(EnvironmentError::InvalidRequest(
                "destination must not be an allowed environment root".to_string(),
            ));
        }
        if src == dst {
            return Err(EnvironmentError::InvalidRequest(
                "source and destination must differ".to_string(),
            ));
        }
        let metadata = std::fs::metadata(&src).map_err(|error| map_io_error(&src, &error))?;
        prepare_local_destination(&dst, overwrite)?;
        if metadata.is_dir() {
            copy_local_dir(&src, &dst)
        } else {
            std::fs::copy(&src, &dst)
                .map(|_| ())
                .map_err(|error| map_io_error(&src, &error))
        }
    }

    async fn write_tmp_file(&self, filename: &str, content: &[u8]) -> EnvironmentResult<String> {
        let normalized = self.tmp_file_relative_path(filename)?;
        let tmp_dir = self.tmp_dir_path().ok_or_else(|| {
            EnvironmentError::Provider("local temporary directory is unavailable".to_string())
        })?;
        let path = tmp_dir.join(&normalized);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
        }
        std::fs::write(&path, content).map_err(|error| map_io_error(&path, &error))?;
        Ok(display_local_path(&path))
    }

    async fn stat(&self, path: &str) -> EnvironmentResult<FileStat> {
        let path = self.resolve(path, false)?;
        let metadata = std::fs::metadata(&path).map_err(|error| map_io_error(&path, &error))?;
        let modified_unix_seconds = metadata
            .modified()
            .ok()
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs());
        Ok(FileStat {
            size: if metadata.is_file() {
                metadata.len()
            } else {
                0
            },
            is_file: metadata.is_file(),
            is_dir: metadata.is_dir(),
            modified_unix_seconds,
        })
    }

    async fn list(&self, path: &str) -> EnvironmentResult<Vec<String>> {
        let path = self.resolve(path, false)?;
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(&path).map_err(|error| map_io_error(&path, &error))? {
            let entry = entry.map_err(|error| EnvironmentError::Provider(error.to_string()))?;
            entries.push(entry.file_name().to_string_lossy().to_string());
        }
        entries.sort();
        Ok(entries)
    }

    async fn glob(
        &self,
        path: &str,
        pattern: &str,
        options: FileGlobOptions,
    ) -> EnvironmentResult<Vec<FileGlobMatch>> {
        let path_glob = PathGlob::new(pattern)?;
        let search_root = self.resolve(path, false)?;
        let builder = local_search_walk_builder(
            &search_root,
            options.include_hidden,
            options.include_ignored,
        );
        let mut glob_matches = Vec::new();
        for entry in builder.build() {
            let entry = entry.map_err(|error| EnvironmentError::Provider(error.to_string()))?;
            if !entry
                .file_type()
                .is_some_and(|file_type| file_type.is_file())
            {
                continue;
            }
            let logical = self.logical_path(entry.path())?;
            if !self.policy.files.permits(&logical, false) {
                continue;
            }
            let candidate = entry
                .path()
                .strip_prefix(&search_root)
                .map(normalize_path)
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
            if path_glob.is_match(&candidate) {
                glob_matches.push(FileGlobMatch { path: logical });
                if options.max_results > 0 && glob_matches.len() >= options.max_results {
                    break;
                }
            }
        }
        Ok(glob_matches)
    }

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
        let path_glob = PathGlob::new(&include)?;
        let search_root = self.resolve(path, false)?;
        let builder = local_search_walk_builder(
            &search_root,
            options.include_hidden,
            options.include_ignored,
        );
        let mut grep_matches = Vec::new();
        let mut searched_files = 0;
        for entry in builder.build() {
            if options.max_results > 0 && grep_matches.len() >= options.max_results {
                break;
            }
            if options.max_files > 0 && searched_files >= options.max_files {
                break;
            }
            let entry = entry.map_err(|error| EnvironmentError::Provider(error.to_string()))?;
            if !entry
                .file_type()
                .is_some_and(|file_type| file_type.is_file())
            {
                continue;
            }
            let logical = self.logical_path(entry.path())?;
            if !self.policy.files.permits(&logical, false) {
                continue;
            }
            let candidate = entry
                .path()
                .strip_prefix(&search_root)
                .map(normalize_path)
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
            if !path_glob.is_match(&candidate) {
                continue;
            }

            searched_files += 1;
            let max_matches = local_grep_file_match_limit(&options, grep_matches.len());
            let mut searcher = SearcherBuilder::new()
                .line_number(true)
                .before_context(options.context_lines)
                .after_context(options.context_lines)
                .binary_detection(BinaryDetection::quit(b'\x00'))
                .max_matches(max_matches)
                .build();
            let mut sink = LocalGrepSink::new(&logical, &mut grep_matches, options.max_results);
            let _ = searcher.search_path(&matcher, entry.path(), &mut sink);
        }
        Ok(grep_matches)
    }

    async fn run_shell(&self, command: ShellCommand) -> EnvironmentResult<ShellOutput> {
        if !self.policy.shell.permits(&command.command) {
            return Err(EnvironmentError::AccessDenied(command.command));
        }
        let cwd = match command.cwd.as_deref() {
            Some(cwd) => self.resolve(cwd, false)?,
            None => self.root.clone(),
        };
        if !cwd.is_dir() {
            return Err(EnvironmentError::InvalidRequest(format!(
                "shell cwd is not a directory: {}",
                cwd.display()
            )));
        }
        run_local_shell_command(
            &command.command,
            &cwd,
            &command.environment,
            command.timeout_seconds,
        )
    }

    async fn get_context_instructions(&self) -> EnvironmentResult<Option<String>> {
        let mut file_trees = Vec::new();
        for allowed_path in &self.allowed_paths {
            let visible_root = self.visible_root_for_allowed_path(allowed_path);
            let tree = generate_local_file_tree(
                allowed_path,
                &visible_root,
                &self.policy,
                DEFAULT_INSTRUCTIONS_MAX_DEPTH,
            )?;
            if !tree.is_empty() && !tree.starts_with("Directory not found") {
                file_trees.push(FileTreeBlock {
                    path: display_local_path(allowed_path),
                    tree,
                });
            }
        }
        Ok(Some(render_environment_context(
            self.id(),
            &display_local_path(&self.root),
            &file_trees,
            self.policy.shell.allow_execute,
            Some(local_shell_metadata()),
        )))
    }

    async fn export_state(&self) -> EnvironmentResult<EnvironmentState> {
        let mut metadata = Metadata::default();
        metadata.insert("root".to_string(), serde_json::json!(self.root));
        metadata.insert(
            "allowed_paths".to_string(),
            serde_json::json!(self.allowed_paths),
        );
        Ok(EnvironmentState {
            provider_id: self.id.clone(),
            files: BTreeMap::new(),
            resources: Vec::new(),
            metadata,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileTreeBlock {
    path: String,
    tree: String,
}

fn render_environment_context(
    provider_id: &str,
    default_directory: &str,
    file_trees: &[FileTreeBlock],
    shell_enabled: bool,
    shell_metadata: Option<ShellMetadata>,
) -> String {
    let mut xml = XmlWriter::new();
    xml.open("environment-context")
        .open("file-system")
        .text_element("provider-id", provider_id)
        .text_element("default-directory", default_directory)
        .open("file-trees");
    for file_tree in file_trees {
        if !file_tree.tree.is_empty() {
            xml.text_block_element_attrs(
                "directory",
                [("path", file_tree.path.as_str())],
                &file_tree.tree,
            );
        }
    }
    xml.close("file-trees").close("file-system");

    if shell_enabled {
        xml.open("shell-execution");
        if let Some(metadata) = shell_metadata {
            xml.text_element("platform", metadata.platform)
                .text_element("shell-type", metadata.shell_type)
                .text_element("shell-executable", metadata.shell_executable);
        }
        xml.close("shell-execution");
    }

    xml.close("environment-context");
    xml.finish()
}

fn generate_virtual_file_tree(
    files: &BTreeMap<String, String>,
    root_path: &str,
    max_depth: usize,
) -> String {
    let root = normalize_file_tree_root(root_path);
    let gitignore = files
        .get(&join_logical_path(&root, ".gitignore"))
        .and_then(|content| build_gitignore(&root, content).ok());
    let entries = collect_virtual_file_tree_entries(files, &root);
    render_file_tree_entries(entries, gitignore.as_ref(), max_depth)
}

fn generate_local_file_tree(
    root: &Path,
    visible_root: &str,
    policy: &EnvironmentPolicy,
    max_depth: usize,
) -> EnvironmentResult<String> {
    if !root.exists() || !root.is_dir() {
        return Ok(format!("Directory not found: {}", root.display()));
    }
    let gitignore = std::fs::read_to_string(root.join(".gitignore"))
        .ok()
        .and_then(|content| build_gitignore(".", &content).ok());
    let mut output = Vec::new();
    collect_local_rendered_file_tree(
        root,
        root,
        visible_root,
        policy,
        gitignore.as_ref(),
        1,
        max_depth,
        &mut output,
    )?;
    Ok(output.join("\n"))
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileTreeEntry {
    path: String,
    is_dir: bool,
}

fn collect_virtual_file_tree_entries(
    files: &BTreeMap<String, String>,
    root: &str,
) -> Vec<FileTreeEntry> {
    let prefix = if root == "." {
        ""
    } else {
        root.trim_matches('/')
    };
    let mut dirs = BTreeSet::<String>::new();
    let mut entries = Vec::new();
    for path in files.keys() {
        if !path_contains(prefix, path) {
            continue;
        }
        let rel = strip_path_prefix(prefix, path);
        if rel.is_empty() {
            continue;
        }
        let normalized = normalize_str_path(rel);
        let mut current = String::new();
        for segment in normalized
            .split('/')
            .collect::<Vec<_>>()
            .iter()
            .take(normalized.split('/').count().saturating_sub(1))
        {
            if !current.is_empty() {
                current.push('/');
            }
            current.push_str(segment);
            dirs.insert(current.clone());
        }
        entries.push(FileTreeEntry {
            path: normalized,
            is_dir: false,
        });
    }
    entries.extend(
        dirs.into_iter()
            .map(|path| FileTreeEntry { path, is_dir: true }),
    );
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    entries
}

#[allow(clippy::too_many_arguments)]
fn collect_local_rendered_file_tree(
    root: &Path,
    current: &Path,
    visible_root: &str,
    policy: &EnvironmentPolicy,
    gitignore: Option<&ignore::gitignore::Gitignore>,
    depth: usize,
    max_depth: usize,
    output: &mut Vec<String>,
) -> EnvironmentResult<()> {
    let mut directories = Vec::new();
    let mut files = Vec::new();
    for child in
        std::fs::read_dir(current).map_err(|error| EnvironmentError::Provider(error.to_string()))?
    {
        let child = child.map_err(|error| EnvironmentError::Provider(error.to_string()))?;
        let path = child.path();
        let relative = normalize_path(
            path.strip_prefix(root)
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?,
        );
        let policy_path = if visible_root.is_empty() {
            relative.clone()
        } else {
            join_logical_path(visible_root, &relative)
        };
        if !policy.files.permits(&policy_path, false) {
            continue;
        }
        let file_type = child
            .file_type()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
        if file_type.is_dir() {
            directories.push((relative, path));
        } else if file_type.is_file() {
            files.push(relative);
        }
    }
    directories.sort_by(|left, right| left.0.cmp(&right.0));
    files.sort();

    for (logical, path) in directories {
        let Some(name) = logical.rsplit('/').next() else {
            continue;
        };
        let (should_skip, should_mark) = should_skip_instruction_item(name, true);
        if should_skip {
            if should_mark {
                output.push(format!("{logical}/ (skipped)"));
            }
            continue;
        }
        if is_gitignored(gitignore, &logical, true) {
            output.push(format!("{logical}/ (gitignored)"));
            continue;
        }
        if depth < max_depth {
            collect_local_rendered_file_tree(
                root,
                &path,
                visible_root,
                policy,
                gitignore,
                depth + 1,
                max_depth,
                output,
            )?;
        }
    }

    for logical in files {
        let Some(name) = logical.rsplit('/').next() else {
            continue;
        };
        let (should_skip, _) = should_skip_instruction_item(name, false);
        if should_skip {
            continue;
        }
        if is_gitignored(gitignore, &logical, false) {
            output.push(format!("{logical} (gitignored)"));
        } else {
            output.push(logical);
        }
    }

    Ok(())
}

fn render_file_tree_entries(
    entries: Vec<FileTreeEntry>,
    gitignore: Option<&ignore::gitignore::Gitignore>,
    max_depth: usize,
) -> String {
    let mut by_parent = BTreeMap::<String, Vec<FileTreeEntry>>::new();
    for entry in entries {
        let parent = parent_path(&entry.path).unwrap_or_default();
        by_parent.entry(parent).or_default().push(entry);
    }
    for children in by_parent.values_mut() {
        children.sort_by(|left, right| match (left.is_dir, right.is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => left.path.cmp(&right.path),
        });
    }
    let mut output = Vec::new();
    collect_rendered_file_tree("", 1, max_depth, &by_parent, gitignore, &mut output);
    output.join("\n")
}

fn collect_rendered_file_tree(
    parent: &str,
    depth: usize,
    max_depth: usize,
    by_parent: &BTreeMap<String, Vec<FileTreeEntry>>,
    gitignore: Option<&ignore::gitignore::Gitignore>,
    output: &mut Vec<String>,
) {
    let Some(children) = by_parent.get(parent) else {
        return;
    };
    for entry in children {
        let Some(name) = entry.path.rsplit('/').next() else {
            continue;
        };
        let (should_skip, should_mark) = should_skip_instruction_item(name, entry.is_dir);
        if should_skip {
            if should_mark {
                output.push(format!("{}/ (skipped)", entry.path));
            }
            continue;
        }
        if is_gitignored(gitignore, &entry.path, entry.is_dir) {
            if entry.is_dir {
                output.push(format!("{}/ (gitignored)", entry.path));
            } else {
                output.push(format!("{} (gitignored)", entry.path));
            }
            continue;
        }
        if entry.is_dir {
            if depth < max_depth {
                collect_rendered_file_tree(
                    &entry.path,
                    depth + 1,
                    max_depth,
                    by_parent,
                    gitignore,
                    output,
                );
            }
        } else {
            output.push(entry.path.clone());
        }
    }
}

fn should_skip_instruction_item(name: &str, is_dir: bool) -> (bool, bool) {
    if is_dir && DEFAULT_INSTRUCTIONS_SKIP_DIRS.contains(&name) {
        return (true, true);
    }
    if !name.starts_with('.') {
        return (false, false);
    }
    if name == ".env" {
        return (false, false);
    }
    (true, false)
}

fn is_gitignored(
    gitignore: Option<&ignore::gitignore::Gitignore>,
    path: &str,
    is_dir: bool,
) -> bool {
    gitignore.is_some_and(|matcher| matcher.matched(path, is_dir).is_ignore())
}

fn build_gitignore(
    root: &str,
    content: &str,
) -> Result<ignore::gitignore::Gitignore, ignore::Error> {
    let mut builder = ignore::gitignore::GitignoreBuilder::new(root);
    for line in content.lines() {
        builder.add_line(None, line)?;
    }
    builder.build()
}

fn normalize_file_tree_root(root: &str) -> String {
    let normalized = normalize_str_path(root);
    if normalized.is_empty() {
        ".".to_string()
    } else {
        normalized
    }
}

fn join_logical_path(root: &str, child: &str) -> String {
    if root.is_empty() || root == "." {
        child.to_string()
    } else {
        format!("{}/{}", root.trim_end_matches('/'), child)
    }
}

fn parent_path(path: &str) -> Option<String> {
    path.rsplit_once('/').map(|(parent, _)| parent.to_string())
}

fn logical_ancestors(path: &str) -> Vec<String> {
    let mut ancestors = Vec::new();
    let mut current = normalize_str_path(path);
    while let Some(parent) = parent_path(&current) {
        if parent.is_empty() {
            break;
        }
        ancestors.push(parent.clone());
        current = parent;
    }
    ancestors
}

fn replace_logical_prefix(path: &str, src: &str, dst: &str) -> String {
    if path == src {
        return dst.to_string();
    }
    path.strip_prefix(src)
        .and_then(|suffix| suffix.strip_prefix('/'))
        .map_or_else(|| path.to_string(), |suffix| join_logical_path(dst, suffix))
}

fn map_io_error(path: &Path, error: &io::Error) -> EnvironmentError {
    match error.kind() {
        io::ErrorKind::NotFound => EnvironmentError::NotFound(path.display().to_string()),
        io::ErrorKind::PermissionDenied => {
            EnvironmentError::AccessDenied(path.display().to_string())
        }
        _ => EnvironmentError::Provider(error.to_string()),
    }
}

fn create_local_tmp_dir(base_dir: Option<&Path>) -> Option<tempfile::TempDir> {
    let mut builder = tempfile::Builder::new();
    builder.prefix(LOCAL_TMP_DIR_PREFIX);
    match base_dir {
        Some(base_dir) => {
            if std::fs::create_dir_all(base_dir).is_err() {
                return None;
            }
            builder.tempdir_in(base_dir).ok()
        }
        None => builder.tempdir().ok(),
    }
}

fn prepare_local_destination(path: &Path, overwrite: bool) -> EnvironmentResult<()> {
    if path.exists() {
        if !overwrite {
            return Err(EnvironmentError::InvalidRequest(format!(
                "destination already exists: {}",
                path.display()
            )));
        }
        let metadata = std::fs::metadata(path).map_err(|error| map_io_error(path, &error))?;
        if metadata.is_dir() {
            std::fs::remove_dir_all(path).map_err(|error| map_io_error(path, &error))?;
        } else {
            std::fs::remove_file(path).map_err(|error| map_io_error(path, &error))?;
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| map_io_error(parent, &error))?;
    }
    Ok(())
}

fn copy_local_dir(src: &Path, dst: &Path) -> EnvironmentResult<()> {
    std::fs::create_dir_all(dst).map_err(|error| map_io_error(dst, &error))?;
    for entry in std::fs::read_dir(src).map_err(|error| map_io_error(src, &error))? {
        let entry = entry.map_err(|error| EnvironmentError::Provider(error.to_string()))?;
        let source_path = entry.path();
        let destination_path = dst.join(entry.file_name());
        let file_type = entry
            .file_type()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
        if file_type.is_dir() {
            copy_local_dir(&source_path, &destination_path)?;
        } else if file_type.is_file() {
            std::fs::copy(&source_path, &destination_path)
                .map(|_| ())
                .map_err(|error| map_io_error(&source_path, &error))?;
        }
    }
    Ok(())
}

fn shell_process_metadata(command: &ShellCommand) -> Metadata {
    let mut metadata = Metadata::default();
    if let Some(timeout_seconds) = command.timeout_seconds {
        metadata.insert(
            "timeout_seconds".to_string(),
            serde_json::json!(timeout_seconds),
        );
    }
    if let Some(cwd) = &command.cwd {
        metadata.insert("cwd".to_string(), serde_json::json!(cwd));
    }
    if !command.environment.is_empty() {
        metadata.insert(
            "environment".to_string(),
            serde_json::json!(command.environment),
        );
    }
    metadata
}

fn refresh_local_shell_process(
    process_id: &str,
    process: &mut LocalShellProcess,
    kill: bool,
) -> EnvironmentResult<ShellProcessSnapshot> {
    if let Some(snapshot) = &process.completed {
        return Ok(snapshot.clone());
    }
    let status = if kill {
        let _ = process.child.kill();
        Some(
            process
                .child
                .wait()
                .map_err(|error| EnvironmentError::Provider(error.to_string()))?,
        )
    } else {
        process
            .child
            .try_wait()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
    };
    let Some(status) = status else {
        return Ok(ShellProcessSnapshot {
            process_id: process_id.to_string(),
            command: process.command.clone(),
            status: ShellProcessStatus::Running,
            stdout: String::new(),
            stderr: String::new(),
            return_code: None,
            metadata: process.metadata.clone(),
        });
    };
    let stdout_handle = process.stdout_handle.take().ok_or_else(|| {
        EnvironmentError::Provider(format!("stdout reader missing for process: {process_id}"))
    })?;
    let stderr_handle = process.stderr_handle.take().ok_or_else(|| {
        EnvironmentError::Provider(format!("stderr reader missing for process: {process_id}"))
    })?;
    let snapshot = ShellProcessSnapshot {
        process_id: process_id.to_string(),
        command: process.command.clone(),
        status: if kill {
            ShellProcessStatus::Killed
        } else if status.success() {
            ShellProcessStatus::Completed
        } else {
            ShellProcessStatus::Failed
        },
        stdout: join_pipe_reader(stdout_handle)?,
        stderr: join_pipe_reader(stderr_handle)?,
        return_code: status.code(),
        metadata: process.metadata.clone(),
    };
    process.completed = Some(snapshot.clone());
    Ok(snapshot)
}

fn run_local_shell_command(
    command: &str,
    cwd: &Path,
    environment: &BTreeMap<String, String>,
    timeout_seconds: Option<u64>,
) -> EnvironmentResult<ShellOutput> {
    let mut child = Command::new("/bin/sh")
        .arg("-lc")
        .arg(command)
        .current_dir(cwd)
        .envs(environment)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
    let mut stdout_reader = child.stdout.take();
    let mut stderr_reader = child.stderr.take();
    let stdout_handle = thread::spawn(move || read_child_pipe(stdout_reader.take()));
    let stderr_handle = thread::spawn(move || read_child_pipe(stderr_reader.take()));

    let mut timed_out = false;
    let status = if let Some(seconds) = timeout_seconds {
        let deadline = Instant::now() + Duration::from_secs(seconds);
        loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() >= deadline => {
                    timed_out = true;
                    let _ = child.kill();
                    break child
                        .wait()
                        .map_err(|error| EnvironmentError::Provider(error.to_string()))?;
                }
                Ok(None) => thread::sleep(Duration::from_millis(25)),
                Err(error) => return Err(EnvironmentError::Provider(error.to_string())),
            }
        }
    } else {
        child
            .wait()
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
    };

    let stdout = join_pipe_reader(stdout_handle)?;
    let mut stderr = join_pipe_reader(stderr_handle)?;
    let mut metadata = Metadata::default();
    if timed_out {
        metadata.insert("timed_out".to_string(), serde_json::json!(true));
        metadata.insert(
            "timeout_seconds".to_string(),
            serde_json::json!(timeout_seconds),
        );
        if !stderr.is_empty() && !stderr.ends_with('\n') {
            stderr.push('\n');
        }
        stderr.push_str("shell command timed out");
    }
    Ok(ShellOutput {
        status: status.code().unwrap_or(-1),
        stdout,
        stderr,
        metadata,
    })
}

fn read_child_pipe(pipe: Option<impl Read>) -> io::Result<String> {
    let mut output = String::new();
    if let Some(mut pipe) = pipe {
        pipe.read_to_string(&mut output)?;
    }
    Ok(output)
}

fn join_pipe_reader(handle: thread::JoinHandle<io::Result<String>>) -> EnvironmentResult<String> {
    handle
        .join()
        .map_err(|_| EnvironmentError::Provider("failed to join shell output reader".to_string()))?
        .map_err(|error| EnvironmentError::Provider(error.to_string()))
}

struct ShellMetadata {
    platform: &'static str,
    shell_type: String,
    shell_executable: String,
}

fn local_shell_metadata() -> ShellMetadata {
    let shell_executable = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    let shell_type = Path::new(&shell_executable).file_name().map_or_else(
        || "sh".to_string(),
        |name| name.to_string_lossy().to_string(),
    );
    ShellMetadata {
        platform: std::env::consts::OS,
        shell_type,
        shell_executable,
    }
}

struct PathGlob {
    matcher: GlobMatcher,
    recursive_prefix_matcher: Option<GlobMatcher>,
    pattern: String,
    anchored: bool,
}

impl PathGlob {
    fn new(pattern: &str) -> EnvironmentResult<Self> {
        let mut normalized = pattern.replace('\\', "/");
        if normalized.is_empty() {
            normalized = "**/*".to_string();
        }
        if let Some(stripped) = normalized.strip_prefix("./") {
            normalized = stripped.to_string();
        }
        let anchored = normalized.starts_with('/');
        let glob_pattern = if anchored {
            let stripped = normalized.trim_start_matches('/');
            if stripped.is_empty() {
                "*"
            } else {
                stripped
            }
        } else {
            normalized.as_str()
        };
        let matcher = compile_glob(glob_pattern)?;
        let recursive_prefix_matcher = glob_pattern
            .strip_prefix("**/")
            .map(compile_glob)
            .transpose()?;
        Ok(Self {
            matcher,
            recursive_prefix_matcher,
            pattern: glob_pattern.to_string(),
            anchored,
        })
    }

    fn is_match(&self, path: &str) -> bool {
        let normalized = normalize_str_path(path);
        if self.anchored && !self.pattern.contains('/') && normalized.contains('/') {
            return false;
        }
        if self.pattern == "**" || self.pattern == "**/*" {
            return true;
        }
        if self.matcher.is_match(&normalized) {
            return true;
        }
        if let Some(matcher) = &self.recursive_prefix_matcher {
            if matcher.is_match(&normalized) {
                return true;
            }
        }
        if !self.anchored && !self.pattern.contains('/') {
            if let Some(name) = normalized.rsplit('/').next() {
                return self.matcher.is_match(name);
            }
        }
        false
    }
}

fn compile_glob(pattern: &str) -> EnvironmentResult<GlobMatcher> {
    GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .map_err(|error| EnvironmentError::InvalidRequest(error.to_string()))
        .map(|glob| glob.compile_matcher())
}

/// Normalize a provider path for glob-style matching.
#[must_use]
pub fn normalize_match_path(path: &str) -> String {
    normalize_str_path(path)
}

/// Return default path candidates for provider-scoped policy matching.
#[must_use]
pub fn path_match_candidates(path: &str) -> Vec<String> {
    let mut candidates = Vec::new();
    push_unique_candidate(&mut candidates, path.replace('\\', "/"));
    push_unique_candidate(&mut candidates, normalize_match_path(path));
    candidates
}

/// Match one path candidate against a relaxed-view pattern.
///
/// Patterns prefixed with `re:` are interpreted as regular expressions;
/// all other patterns use the same ripgrep-style glob semantics as provider
/// glob searches.
///
/// # Errors
///
/// Returns an error when the glob or regular expression pattern is invalid.
pub fn matches_path_pattern(path: &str, pattern: &str) -> EnvironmentResult<bool> {
    let pattern = pattern.trim();
    if let Some(regex) = pattern.strip_prefix("re:") {
        let matcher = RegexMatcher::new(regex)
            .map_err(|error| EnvironmentError::InvalidRequest(error.to_string()))?;
        return matcher
            .is_match(path.as_bytes())
            .map_err(|error| EnvironmentError::Provider(error.to_string()));
    }
    PathGlob::new(pattern).map(|path_glob| path_glob.is_match(path))
}

fn push_unique_candidate(candidates: &mut Vec<String>, candidate: String) {
    if !candidate.is_empty() && !candidates.iter().any(|existing| existing == &candidate) {
        candidates.push(candidate);
    }
}

fn search_text(
    path: &str,
    content: &str,
    regex_matcher: &RegexMatcher,
    context_lines: usize,
    max_matches_per_file: usize,
    max_results: usize,
    grep_matches: &mut Vec<FileGrepMatch>,
) -> EnvironmentResult<()> {
    let lines: Vec<&str> = content.split_inclusive('\n').collect();
    let mut file_matches = 0;
    for (index, line) in lines.iter().enumerate() {
        if max_results > 0 && grep_matches.len() >= max_results {
            break;
        }
        if max_matches_per_file > 0 && file_matches >= max_matches_per_file {
            break;
        }
        if regex_matcher
            .is_match(line.as_bytes())
            .map_err(|error| EnvironmentError::Provider(error.to_string()))?
        {
            let start_index = index.saturating_sub(context_lines);
            let end_index = (index + context_lines + 1).min(lines.len());
            grep_matches.push(FileGrepMatch {
                path: path.to_string(),
                line_number: index + 1,
                matching_line: line.trim_end_matches('\n').to_string(),
                context: lines[start_index..end_index].concat(),
                context_start_line: start_index + 1,
            });
            file_matches += 1;
        }
    }
    Ok(())
}

fn local_search_walk_builder(
    search_root: &Path,
    include_hidden: bool,
    include_ignored: bool,
) -> ignore::WalkBuilder {
    let mut builder = ignore::WalkBuilder::new(search_root);
    builder.hidden(!include_hidden);
    builder.ignore(!include_ignored);
    builder.git_ignore(!include_ignored);
    builder.git_global(!include_ignored);
    builder.git_exclude(!include_ignored);
    builder.require_git(false);
    builder.follow_links(false);
    builder
}

fn local_grep_file_match_limit(options: &FileGrepOptions, current_matches: usize) -> Option<u64> {
    let remaining_total = options
        .max_results
        .checked_sub(current_matches)
        .filter(|_| options.max_results > 0);
    match (options.max_matches_per_file, remaining_total) {
        (0, None) => None,
        (0, Some(remaining)) => Some(remaining as u64),
        (per_file, None) => Some(per_file as u64),
        (per_file, Some(remaining)) => Some(per_file.min(remaining) as u64),
    }
}

struct LocalGrepSink<'a> {
    path: &'a str,
    grep_matches: &'a mut Vec<FileGrepMatch>,
    max_results: usize,
    pending_before_context: Vec<(usize, String)>,
    active_match_index: Option<usize>,
}

impl<'a> LocalGrepSink<'a> {
    fn new(path: &'a str, grep_matches: &'a mut Vec<FileGrepMatch>, max_results: usize) -> Self {
        Self {
            path,
            grep_matches,
            max_results,
            pending_before_context: Vec::new(),
            active_match_index: None,
        }
    }

    fn line_string(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }

    fn line_number(line_number: Option<u64>) -> usize {
        line_number
            .and_then(|line_number| usize::try_from(line_number).ok())
            .unwrap_or(1)
    }

    fn should_accept_match(&self) -> bool {
        self.max_results == 0 || self.grep_matches.len() < self.max_results
    }
}

impl Sink for LocalGrepSink<'_> {
    type Error = io::Error;

    fn matched(&mut self, _searcher: &Searcher, mat: &SinkMatch<'_>) -> Result<bool, Self::Error> {
        if !self.should_accept_match() {
            return Ok(false);
        }
        let line_number = Self::line_number(mat.line_number());
        let matching_line = Self::line_string(mat.bytes());
        let context_start_line = self
            .pending_before_context
            .first()
            .map_or(line_number, |(line_number, _)| *line_number);
        let mut context = String::new();
        for (_, line) in self.pending_before_context.drain(..) {
            context.push_str(&line);
        }
        context.push_str(&matching_line);
        self.grep_matches.push(FileGrepMatch {
            path: self.path.to_string(),
            line_number,
            matching_line: matching_line.trim_end_matches('\n').to_string(),
            context,
            context_start_line,
        });
        self.active_match_index = Some(self.grep_matches.len() - 1);
        Ok(true)
    }

    fn context(
        &mut self,
        _searcher: &Searcher,
        context: &SinkContext<'_>,
    ) -> Result<bool, Self::Error> {
        let line = Self::line_string(context.bytes());
        match context.kind() {
            grep_searcher::SinkContextKind::Before => {
                self.pending_before_context
                    .push((Self::line_number(context.line_number()), line));
            }
            grep_searcher::SinkContextKind::After | grep_searcher::SinkContextKind::Other => {
                if let Some(index) = self.active_match_index {
                    if let Some(grep_match) = self.grep_matches.get_mut(index) {
                        grep_match.context.push_str(&line);
                    }
                }
            }
        }
        Ok(true)
    }

    fn context_break(&mut self, _searcher: &Searcher) -> Result<bool, Self::Error> {
        self.pending_before_context.clear();
        self.active_match_index = None;
        Ok(true)
    }

    fn binary_data(
        &mut self,
        _searcher: &Searcher,
        _binary_byte_offset: u64,
    ) -> Result<bool, Self::Error> {
        Ok(false)
    }

    fn finish(&mut self, _searcher: &Searcher, _finish: &SinkFinish) -> Result<(), Self::Error> {
        self.pending_before_context.clear();
        self.active_match_index = None;
        Ok(())
    }
}

fn normalize_path(path: &Path) -> String {
    normalize_str_path(&path.to_string_lossy())
}

fn normalize_local_config_path(path: PathBuf) -> PathBuf {
    let absolute = if path.is_absolute() {
        path
    } else {
        std::env::current_dir().map_or_else(|_| path.clone(), |current_dir| current_dir.join(&path))
    };
    absolute.canonicalize().unwrap_or(absolute)
}

fn normalize_absolute_request_path(path: &Path) -> EnvironmentResult<PathBuf> {
    if !path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::Prefix(_) | Component::ParentDir | Component::CurDir
            )
        })
    {
        return Err(EnvironmentError::InvalidRequest(path.display().to_string()));
    }
    Ok(normalize_local_config_path(path.to_path_buf()))
}

fn display_local_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn push_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.iter().any(|existing| existing == &path) {
        paths.push(path);
    }
}

fn normalize_requested_path(path: &str) -> EnvironmentResult<String> {
    let requested = Path::new(path);
    if requested.components().any(|component| {
        matches!(
            component,
            Component::Prefix(_) | Component::RootDir | Component::ParentDir
        )
    }) {
        return Err(EnvironmentError::InvalidRequest(path.to_string()));
    }
    Ok(normalize_str_path(path))
}

fn is_tmp_path(path: &str) -> bool {
    let normalized = normalize_str_path(path);
    normalized == DEFAULT_TMP_DIR || normalized.starts_with(&format!("{DEFAULT_TMP_DIR}/"))
}

fn normalize_tmp_filename(filename: &str) -> EnvironmentResult<String> {
    let normalized = normalize_requested_path(filename)?;
    if normalized.is_empty() {
        return Err(EnvironmentError::InvalidRequest(
            "tmp filename must be non-empty".to_string(),
        ));
    }
    if is_tmp_path(&normalized) {
        return Err(EnvironmentError::InvalidRequest(
            "tmp filename must be relative to the provider tmp directory".to_string(),
        ));
    }
    Ok(normalized)
}

fn normalize_tmp_namespace(namespace: &str) -> EnvironmentResult<String> {
    let normalized = normalize_tmp_filename(namespace)?;
    if normalized.contains('/') {
        return Err(EnvironmentError::InvalidRequest(
            "tmp namespace must be a single path segment".to_string(),
        ));
    }
    Ok(normalized)
}

fn normalize_str_path(path: &str) -> String {
    let mut normalized = path.replace('\\', "/");
    if let Some(stripped) = normalized.strip_prefix("./") {
        normalized = stripped.to_string();
    }
    normalized.trim_start_matches('/').to_string()
}

fn include_path(path: &str, include_hidden: bool) -> bool {
    include_hidden
        || !normalize_str_path(path)
            .split('/')
            .any(|segment| segment.starts_with('.') && segment.len() > 1)
}

fn path_contains(prefix: &str, path: &str) -> bool {
    prefix.is_empty() || path == prefix || path.starts_with(&format!("{prefix}/"))
}

fn strip_path_prefix<'a>(prefix: &str, path: &'a str) -> &'a str {
    if prefix.is_empty() {
        path
    } else {
        path.strip_prefix(prefix)
            .and_then(|value| value.strip_prefix('/'))
            .unwrap_or(path)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[tokio::test]
    async fn virtual_provider_reads_lists_shells_and_exports_state() {
        let output = ShellOutput {
            status: 0,
            stdout: "ok".to_string(),
            stderr: String::new(),
            metadata: Metadata::default(),
        };
        let provider = VirtualEnvironmentProvider::new("test")
            .with_file("src/lib.rs", "content")
            .with_shell_output("echo ok", output.clone());

        assert_eq!(provider.read_text("src/lib.rs").await.unwrap(), "content");
        provider
            .write_text("src/main.rs", "fn main() {}")
            .await
            .unwrap();
        assert_eq!(
            provider.read_text("src/main.rs").await.unwrap(),
            "fn main() {}"
        );
        assert_eq!(
            provider.list("src").await.unwrap(),
            vec!["src/lib.rs", "src/main.rs"]
        );
        assert_eq!(
            provider
                .run_shell(ShellCommand {
                    command: "echo ok".to_string(),
                    ..ShellCommand::default()
                })
                .await
                .unwrap(),
            output
        );
        let state = provider.export_state().await.unwrap();
        assert_eq!(state.provider_id, "test");
        assert_eq!(state.files["src/main.rs"], "fn main() {}");
    }

    #[tokio::test]
    async fn virtual_provider_globs_and_greps_with_native_matchers() {
        let provider = VirtualEnvironmentProvider::new("test")
            .with_file("src/lib.rs", "pub fn library() {}\n")
            .with_file("src/main.rs", "fn main() { library(); }\n")
            .with_file("README.md", "library docs\n");

        let glob_matches = provider
            .glob("", "*.rs", FileGlobOptions::default())
            .await
            .unwrap();
        assert_eq!(
            glob_matches
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            vec!["src/lib.rs", "src/main.rs"]
        );

        let grep_matches = provider
            .grep(
                "",
                "library",
                FileGrepOptions {
                    include: Some("**/*.rs".to_string()),
                    context_lines: 0,
                    max_results: 10,
                    max_matches_per_file: 10,
                    max_files: 50,
                    include_hidden: false,
                    include_ignored: false,
                },
            )
            .await
            .unwrap();
        assert_eq!(grep_matches.len(), 2);
        assert_eq!(grep_matches[0].path, "src/lib.rs");
        assert_eq!(grep_matches[0].line_number, 1);
    }

    #[test]
    fn path_glob_matches_ripgrep_style_patterns() {
        let bare = PathGlob::new("*.rs").unwrap();
        assert!(bare.is_match("lib.rs"));
        assert!(bare.is_match("src/lib.rs"));
        assert!(!bare.is_match("src/lib.py"));

        let recursive = PathGlob::new("**/*.rs").unwrap();
        assert!(recursive.is_match("lib.rs"));
        assert!(recursive.is_match("src/lib.rs"));

        let anchored_file = PathGlob::new("/*.rs").unwrap();
        assert!(anchored_file.is_match("lib.rs"));
        assert!(!anchored_file.is_match("src/lib.rs"));

        let scoped_dir = PathGlob::new("src/*.rs").unwrap();
        assert!(scoped_dir.is_match("src/lib.rs"));
        assert!(!scoped_dir.is_match("src/nested/mod.rs"));

        let anchored_dir = PathGlob::new("/src/*.rs").unwrap();
        assert!(anchored_dir.is_match("src/lib.rs"));
        assert!(!anchored_dir.is_match("src/nested/mod.rs"));
        assert!(!anchored_dir.is_match("nested/src/lib.rs"));
    }

    #[tokio::test]
    async fn virtual_provider_search_respects_root_hidden_limits_and_invalid_patterns() {
        let provider = VirtualEnvironmentProvider::new("test")
            .with_file("src/lib.rs", "alpha\nbeta\nalpha again\n")
            .with_file("src/nested/mod.rs", "alpha nested\n")
            .with_file("tests/lib.rs", "alpha test\n")
            .with_file("src/.hidden.rs", "alpha hidden\n")
            .with_file("README.md", "alpha docs\n");

        let src_matches = provider
            .glob("src", "*.rs", FileGlobOptions::default())
            .await
            .unwrap();
        assert_eq!(
            src_matches
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            vec!["src/lib.rs", "src/nested/mod.rs"]
        );

        let hidden_default = provider
            .glob("src", ".*.rs", FileGlobOptions::default())
            .await
            .unwrap();
        assert!(hidden_default.is_empty());

        let hidden_included = provider
            .glob(
                "src",
                ".*.rs",
                FileGlobOptions {
                    include_hidden: true,
                    ..FileGlobOptions::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(hidden_included[0].path, "src/.hidden.rs");

        let limited = provider
            .glob(
                "",
                "*.rs",
                FileGlobOptions {
                    max_results: 1,
                    ..FileGlobOptions::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(limited.len(), 1);

        let grep_matches = provider
            .grep(
                "src",
                "alpha",
                FileGrepOptions {
                    include: Some("**/*.rs".to_string()),
                    context_lines: 1,
                    max_results: 2,
                    max_matches_per_file: 1,
                    max_files: 50,
                    include_hidden: false,
                    include_ignored: false,
                },
            )
            .await
            .unwrap();
        assert_eq!(grep_matches.len(), 2);
        assert_eq!(grep_matches[0].path, "src/lib.rs");
        assert_eq!(grep_matches[0].line_number, 1);
        assert_eq!(grep_matches[0].context_start_line, 1);
        assert!(grep_matches[0].context.contains("beta"));
        assert_eq!(grep_matches[1].path, "src/nested/mod.rs");

        assert!(matches!(
            provider.grep("", "(", FileGrepOptions::default()).await,
            Err(EnvironmentError::InvalidRequest(_))
        ));
        assert!(matches!(
            provider.glob("", "[", FileGlobOptions::default()).await,
            Err(EnvironmentError::InvalidRequest(_))
        ));
    }

    #[tokio::test]
    async fn local_provider_search_respects_gitignore_hidden_and_policy() {
        let root = unique_test_dir();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "needle\n").unwrap();
        std::fs::write(root.join("src/ignored.log"), "needle ignored\n").unwrap();
        std::fs::write(root.join(".hidden.rs"), "needle hidden\n").unwrap();
        std::fs::write(root.join(".gitignore"), "*.log\n").unwrap();

        let provider = LocalEnvironmentProvider::new(&root).with_policy(EnvironmentPolicy {
            files: FilePolicy::read_only(),
            shell: ShellPolicy::default(),
        });

        let visible = provider
            .glob("", "**/*", FileGlobOptions::default())
            .await
            .unwrap();
        let visible_paths = visible
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>();
        assert!(visible_paths.contains(&"src/lib.rs"));
        assert!(!visible_paths.contains(&"src/ignored.log"));
        assert!(!visible_paths.contains(&".hidden.rs"));

        let all_files = provider
            .glob(
                "",
                "**/*",
                FileGlobOptions {
                    include_hidden: true,
                    include_ignored: true,
                    max_results: 0,
                },
            )
            .await
            .unwrap();
        let all_paths = all_files
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>();
        assert!(all_paths.contains(&"src/ignored.log"));
        assert!(all_paths.contains(&".hidden.rs"));

        let grep_matches = provider
            .grep(
                "",
                "needle",
                FileGrepOptions {
                    include: Some("**/*".to_string()),
                    include_hidden: true,
                    include_ignored: true,
                    max_results: 0,
                    max_matches_per_file: 0,
                    max_files: 0,
                    context_lines: 0,
                },
            )
            .await
            .unwrap();
        assert_eq!(grep_matches.len(), 3);

        let restricted = provider.with_policy(EnvironmentPolicy {
            files: FilePolicy {
                allow_read: true,
                allow_write: false,
                allowed_prefixes: vec!["src".to_string()],
            },
            shell: ShellPolicy::default(),
        });
        assert!(matches!(
            restricted
                .glob("README.md", "**/*", FileGlobOptions::default())
                .await,
            Err(EnvironmentError::AccessDenied(_))
        ));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn local_provider_grep_streams_context_limits_and_binary_detection() {
        let root = unique_test_dir();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/a.txt"), "before\nneedle one\nafter\n").unwrap();
        std::fs::write(root.join("src/b.txt"), "needle two\nneedle three\n").unwrap();
        std::fs::write(root.join("src/binary.bin"), b"needle\0binary\n").unwrap();

        let provider = LocalEnvironmentProvider::new(&root).with_policy(EnvironmentPolicy {
            files: FilePolicy::read_only(),
            shell: ShellPolicy::default(),
        });

        let context_matches = provider
            .grep(
                "",
                "needle one",
                FileGrepOptions {
                    include: Some("**/*.txt".to_string()),
                    include_hidden: false,
                    include_ignored: false,
                    max_results: 10,
                    max_matches_per_file: 10,
                    max_files: 10,
                    context_lines: 1,
                },
            )
            .await
            .unwrap();
        assert_eq!(context_matches.len(), 1);
        assert_eq!(context_matches[0].path, "src/a.txt");
        assert_eq!(context_matches[0].line_number, 2);
        assert_eq!(context_matches[0].matching_line, "needle one");
        assert_eq!(context_matches[0].context_start_line, 1);
        assert_eq!(context_matches[0].context, "before\nneedle one\nafter\n");

        let limited_matches = provider
            .grep(
                "",
                "needle",
                FileGrepOptions {
                    include: Some("**/*.txt".to_string()),
                    include_hidden: false,
                    include_ignored: false,
                    max_results: 10,
                    max_matches_per_file: 1,
                    max_files: 10,
                    context_lines: 0,
                },
            )
            .await
            .unwrap();
        assert_eq!(
            limited_matches
                .iter()
                .filter(|entry| entry.path == "src/b.txt")
                .count(),
            1
        );

        let binary_skipped = provider
            .grep(
                "",
                "binary",
                FileGrepOptions {
                    include: Some("**/*".to_string()),
                    include_hidden: true,
                    include_ignored: true,
                    max_results: 10,
                    max_matches_per_file: 10,
                    max_files: 10,
                    context_lines: 0,
                },
            )
            .await
            .unwrap();
        assert!(binary_skipped.is_empty());

        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn local_provider_runs_background_shell_processes() {
        let root = unique_test_dir();
        std::fs::create_dir_all(&root).unwrap();
        let provider = LocalEnvironmentProvider::new(&root).with_policy(EnvironmentPolicy {
            files: FilePolicy::read_write(),
            shell: ShellPolicy::allow_all(),
        });

        let started = provider
            .start_process(ShellCommand {
                command: "printf ready".to_string(),
                timeout_seconds: Some(5),
                ..ShellCommand::default()
            })
            .await
            .unwrap();
        assert_eq!(started.status, ShellProcessStatus::Running);
        assert_eq!(started.command, "printf ready");
        assert_eq!(started.metadata["timeout_seconds"], serde_json::json!(5));

        let completed = provider.wait_process(&started.process_id, 5).await.unwrap();
        assert_eq!(completed.status, ShellProcessStatus::Completed);
        assert_eq!(completed.stdout, "ready");
        assert_eq!(completed.return_code, Some(0));

        let listed = provider.list_processes().await.unwrap();
        assert!(listed.iter().any(|process| {
            process.process_id == started.process_id
                && process.status == ShellProcessStatus::Completed
        }));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn local_provider_manages_tmp_files_as_allowed_absolute_paths() {
        let root = unique_test_dir();
        let external = unique_test_dir();
        let unrelated_tmp = std::env::temp_dir().join(format!(
            "starweaver-unrelated-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::write(&unrelated_tmp, "secret").unwrap();
        let provider = LocalEnvironmentProvider::new(&root)
            .with_allowed_paths([external.clone()])
            .with_policy(EnvironmentPolicy {
                files: FilePolicy::read_only(),
                shell: ShellPolicy::default(),
            });

        let tmp_path = provider
            .write_tmp_file("stdout.log", b"full shell output")
            .await
            .unwrap();
        let tmp_path_buf = PathBuf::from(&tmp_path);
        assert!(tmp_path_buf.is_absolute());
        assert!(provider.path_is_managed_tmp(&tmp_path_buf));
        assert!(provider
            .allowed_paths()
            .iter()
            .any(|path| tmp_path_buf.starts_with(path)));
        assert_eq!(
            provider.read_text(&tmp_path).await.unwrap(),
            "full shell output"
        );
        assert!(!root.join(".starweaver/tmp/stdout.log").exists());
        assert_eq!(
            provider
                .read_text(".starweaver/tmp/stdout.log")
                .await
                .unwrap(),
            "full shell output"
        );
        assert!(matches!(
            provider
                .read_text(&unrelated_tmp.display().to_string())
                .await,
            Err(EnvironmentError::AccessDenied(_))
        ));

        let _ = std::fs::remove_file(unrelated_tmp);
        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(external).unwrap();
    }

    #[tokio::test]
    async fn local_provider_tmp_namespace_isolates_managed_tmp_files() {
        let root = unique_test_dir();
        let provider = LocalEnvironmentProvider::new(&root)
            .with_tmp_namespace("session_123")
            .with_policy(EnvironmentPolicy {
                files: FilePolicy::read_only(),
                shell: ShellPolicy::default(),
            });

        let tmp_path = provider.write_tmp_file("grep.json", b"[]").await.unwrap();
        let tmp_path_buf = PathBuf::from(&tmp_path);
        assert!(tmp_path_buf.ends_with("session_123/grep.json"));
        assert_eq!(provider.read_text(&tmp_path).await.unwrap(), "[]");
        assert_eq!(
            provider
                .read_text(".starweaver/tmp/session_123/grep.json")
                .await
                .unwrap(),
            "[]"
        );
        assert!(tmp_path_buf
            .parent()
            .is_some_and(|parent| parent.file_name().is_some_and(|name| name == "session_123")));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn virtual_provider_tmp_namespace_isolates_tmp_files() {
        let provider = VirtualEnvironmentProvider::new("virtual").with_tmp_namespace("session_123");

        let tmp_path = provider.write_tmp_file("grep.json", b"[]").await.unwrap();
        assert_eq!(tmp_path, ".starweaver/tmp/session_123/grep.json");
        assert_eq!(provider.read_text(&tmp_path).await.unwrap(), "[]");
        assert!(matches!(
            provider.read_text(".starweaver/tmp/grep.json").await,
            Err(EnvironmentError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn local_provider_tmp_base_dir_places_managed_tmp_under_base() {
        let root = unique_test_dir();
        let tmp_base = unique_test_dir();
        let provider = LocalEnvironmentProvider::new(&root)
            .with_tmp_base_dir(tmp_base.clone())
            .with_policy(EnvironmentPolicy {
                files: FilePolicy::read_only(),
                shell: ShellPolicy::default(),
            });

        let tmp_dir = provider.tmp_dir_path().unwrap().to_path_buf();
        assert!(tmp_dir.starts_with(&tmp_base));
        assert!(tmp_dir
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with(LOCAL_TMP_DIR_PREFIX));
        let tmp_path = provider.write_tmp_file("grep.json", b"[]").await.unwrap();
        assert!(PathBuf::from(&tmp_path).starts_with(&tmp_dir));
        assert_eq!(provider.read_text(&tmp_path).await.unwrap(), "[]");

        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(tmp_base).unwrap();
    }

    #[tokio::test]
    async fn local_provider_search_preserves_gitignore_negations() {
        let root = unique_test_dir();
        std::fs::create_dir_all(root.join("ignored")).unwrap();
        std::fs::create_dir_all(root.join("other_ignored")).unwrap();
        std::fs::write(root.join("ignored/keep.txt"), "needle keep\n").unwrap();
        std::fs::write(root.join("ignored/drop.txt"), "needle drop\n").unwrap();
        std::fs::write(root.join("other_ignored/drop.txt"), "needle other\n").unwrap();
        std::fs::write(
            root.join(".gitignore"),
            "ignored/*\nother_ignored/\n!ignored/keep.txt\n",
        )
        .unwrap();

        let provider = LocalEnvironmentProvider::new(&root).with_policy(EnvironmentPolicy {
            files: FilePolicy::read_only(),
            shell: ShellPolicy::default(),
        });

        let glob_matches = provider
            .glob(
                "",
                "**/*.txt",
                FileGlobOptions {
                    max_results: 0,
                    ..FileGlobOptions::default()
                },
            )
            .await
            .unwrap();
        let glob_paths = glob_matches
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>();
        assert!(glob_paths.contains(&"ignored/keep.txt"));
        assert!(!glob_paths.contains(&"ignored/drop.txt"));
        assert!(!glob_paths.contains(&"other_ignored/drop.txt"));

        let grep_matches = provider
            .grep(
                "",
                "needle",
                FileGrepOptions {
                    include: Some("**/*.txt".to_string()),
                    include_hidden: false,
                    include_ignored: false,
                    max_results: 0,
                    max_matches_per_file: 0,
                    max_files: 0,
                    context_lines: 0,
                },
            )
            .await
            .unwrap();
        assert_eq!(grep_matches.len(), 1);
        assert_eq!(grep_matches[0].path, "ignored/keep.txt");

        let include_ignored = provider
            .glob(
                "",
                "**/*.txt",
                FileGlobOptions {
                    include_hidden: false,
                    include_ignored: true,
                    max_results: 0,
                },
            )
            .await
            .unwrap();
        let include_ignored_paths = include_ignored
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>();
        assert!(include_ignored_paths.contains(&"ignored/drop.txt"));
        assert!(include_ignored_paths.contains(&"other_ignored/drop.txt"));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn virtual_context_file_tree_matches_starweaver_sdk_semantics() {
        let provider = VirtualEnvironmentProvider::new("test")
            .with_file(".git/config", "git")
            .with_file(".gitignore", "*.log\nbuild/\n")
            .with_file(".hidden/secret.txt", "secret")
            .with_file(".env", "ENV=value")
            .with_file("README.md", "readme")
            .with_file("build/output.js", "built")
            .with_file("error.log", "log")
            .with_file("level1/level2/level3/file.txt", "too deep")
            .with_file("node_modules/package.json", "{}")
            .with_file("src/main.rs", "fn main() {}")
            .with_policy(EnvironmentPolicy {
                files: FilePolicy::read_only(),
                shell: ShellPolicy::default(),
            });

        let instructions = provider.get_context_instructions().await.unwrap().unwrap();

        assert!(instructions.contains("<environment-context>"));
        assert!(instructions.contains("<file-system>"));
        assert!(instructions.contains("<file-trees>"));
        assert!(instructions.contains("<directory path=\".\">"));
        assert!(!instructions.contains("<file>"));
        assert!(instructions.contains(".git/ (skipped)"));
        assert!(instructions.contains("node_modules/ (skipped)"));
        assert!(instructions.contains("build/ (gitignored)"));
        assert!(instructions.contains("error.log (gitignored)"));
        assert!(instructions.contains(".env"));
        assert!(instructions.contains("README.md"));
        assert!(instructions.contains("src/main.rs"));
        assert!(!instructions.contains(".hidden"));
        assert!(!instructions.contains(".gitignore"));
        assert!(!instructions.contains("package.json"));
        assert!(!instructions.contains("build/output.js"));
        assert!(!instructions.contains("level1/level2/level3/file.txt"));
    }

    #[tokio::test]
    async fn local_context_file_tree_matches_starweaver_sdk_semantics() {
        let root = unique_test_dir();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join(".hidden")).unwrap();
        std::fs::create_dir_all(root.join("build")).unwrap();
        std::fs::create_dir_all(root.join("level1/level2/level3")).unwrap();
        std::fs::create_dir_all(root.join("node_modules")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join(".git/config"), "git").unwrap();
        std::fs::write(root.join(".gitignore"), "*.log\nbuild/\n").unwrap();
        std::fs::write(root.join(".hidden/secret.txt"), "secret").unwrap();
        std::fs::write(root.join(".env"), "ENV=value").unwrap();
        std::fs::write(root.join("README.md"), "readme").unwrap();
        std::fs::write(root.join("build/output.js"), "built").unwrap();
        std::fs::write(root.join("error.log"), "log").unwrap();
        std::fs::write(root.join("level1/level2/level3/file.txt"), "too deep").unwrap();
        std::fs::write(root.join("node_modules/package.json"), "{}").unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();

        let provider = LocalEnvironmentProvider::new(&root).with_policy(EnvironmentPolicy {
            files: FilePolicy::read_only(),
            shell: ShellPolicy::default(),
        });
        let instructions = provider.get_context_instructions().await.unwrap().unwrap();

        assert!(instructions.contains(&format!("<directory path=\"{}\">", root.display())));
        assert!(!instructions.contains("<file>"));
        assert!(instructions.contains(".git/ (skipped)"));
        assert!(instructions.contains("node_modules/ (skipped)"));
        assert!(instructions.contains("build/ (gitignored)"));
        assert!(instructions.contains("error.log (gitignored)"));
        assert!(instructions.contains(".env"));
        assert!(instructions.contains("README.md"));
        assert!(instructions.contains("src/main.rs"));
        assert!(!instructions.contains(".hidden"));
        assert!(!instructions.contains(".gitignore"));
        assert!(!instructions.contains("package.json"));
        assert!(!instructions.contains("build/output.js"));
        assert!(!instructions.contains("level1/level2/level3/file.txt"));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn local_provider_accepts_allowed_absolute_paths_and_rejects_unsafe_paths() {
        let root = unique_test_dir();
        let external = unique_test_dir();
        std::fs::create_dir_all(external.join("research")).unwrap();
        std::fs::write(root.join("safe..name.txt"), "ok").unwrap();
        std::fs::write(external.join("research/SKILL.md"), "skill").unwrap();
        let provider = LocalEnvironmentProvider::new(&root)
            .with_allowed_paths([external.clone()])
            .with_policy(EnvironmentPolicy {
                files: FilePolicy::read_only(),
                shell: ShellPolicy::default(),
            });

        assert_eq!(
            provider.read_text("safe..name.txt").await.unwrap(),
            "ok".to_string()
        );
        assert_eq!(
            provider
                .read_text(&external.join("research/SKILL.md").display().to_string())
                .await
                .unwrap(),
            "skill".to_string()
        );
        assert_eq!(
            provider
                .list(&external.display().to_string())
                .await
                .unwrap(),
            vec!["research"]
        );
        let matches = provider
            .glob(
                &external.display().to_string(),
                "*/SKILL.md",
                FileGlobOptions {
                    include_hidden: true,
                    include_ignored: true,
                    max_results: 0,
                },
            )
            .await
            .unwrap();
        assert_eq!(
            matches,
            vec![FileGlobMatch {
                path: display_local_path(&external.join("research/SKILL.md")),
            }]
        );
        assert!(matches!(
            provider.read_text("/etc/passwd").await,
            Err(EnvironmentError::AccessDenied(_))
        ));
        assert!(matches!(
            provider.read_text("../outside.txt").await,
            Err(EnvironmentError::InvalidRequest(_))
        ));
        assert!(matches!(
            provider
                .read_text(&format!("{}/../outside.txt", external.display()))
                .await,
            Err(EnvironmentError::InvalidRequest(_))
        ));

        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(external).unwrap();
    }

    #[tokio::test]
    async fn local_context_file_tree_includes_allowed_external_roots() {
        let root = unique_test_dir();
        let external = unique_test_dir();
        std::fs::create_dir_all(external.join("skills/research")).unwrap();
        std::fs::write(root.join("README.md"), "readme").unwrap();
        std::fs::write(external.join("skills/research/SKILL.md"), "skill").unwrap();
        let provider = LocalEnvironmentProvider::new(&root)
            .with_allowed_paths([external.clone()])
            .with_policy(EnvironmentPolicy {
                files: FilePolicy::read_only(),
                shell: ShellPolicy::default(),
            });
        let instructions = provider.get_context_instructions().await.unwrap().unwrap();

        assert!(instructions.contains(&format!(
            "<default-directory>{}</default-directory>",
            root.display()
        )));
        assert!(instructions.contains(&format!("<directory path=\"{}\">", root.display())));
        assert!(instructions.contains(&format!("<directory path=\"{}\">", external.display())));
        assert!(instructions.contains("README.md"));
        assert!(instructions.contains("skills/research/SKILL.md"));

        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(external).unwrap();
    }

    #[tokio::test]
    async fn local_provider_runs_shell_with_cwd_environment_and_policy() {
        let root = unique_test_dir();
        std::fs::create_dir_all(root.join("work")).unwrap();
        std::fs::write(root.join("work/input.txt"), "content").unwrap();
        let provider = LocalEnvironmentProvider::new(&root).with_policy(EnvironmentPolicy {
            files: FilePolicy::read_only(),
            shell: ShellPolicy::allow_all(),
        });

        let output = provider
            .run_shell(ShellCommand {
                command: "printf '%s:%s' \"$STARWEAVER_TEST\" \"$(pwd | sed 's#.*/##')\""
                    .to_string(),
                cwd: Some("work".to_string()),
                environment: BTreeMap::from([("STARWEAVER_TEST".to_string(), "ok".to_string())]),
                ..ShellCommand::default()
            })
            .await
            .unwrap();
        assert_eq!(output.status, 0);
        assert_eq!(output.stdout, "ok:work");

        let denied = LocalEnvironmentProvider::new(&root).with_policy(EnvironmentPolicy {
            files: FilePolicy::read_only(),
            shell: ShellPolicy::default(),
        });
        assert!(matches!(
            denied
                .run_shell(ShellCommand {
                    command: "echo denied".to_string(),
                    ..ShellCommand::default()
                })
                .await,
            Err(EnvironmentError::AccessDenied(_))
        ));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn policy_denies_disallowed_file_access() {
        let provider = VirtualEnvironmentProvider::new("test").with_policy(EnvironmentPolicy {
            files: FilePolicy::default(),
            shell: ShellPolicy::default(),
        });
        assert!(matches!(
            provider.read_text("secret").await,
            Err(EnvironmentError::AccessDenied(_))
        ));
    }

    fn unique_test_dir() -> PathBuf {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "starweaver-env-test-{}-{:?}-{suffix}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path.canonicalize().unwrap_or(path)
    }
}
