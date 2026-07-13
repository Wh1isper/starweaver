//! Shared request, response, search, shell, and state types for environment providers.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use starweaver_core::{
    Metadata, VersionedRecord, VersionedRecordError, from_versioned_value, to_versioned_value,
};

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

/// Directory list options for provider-backed filesystem listing.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileListOptions {
    /// Entry name patterns to ignore.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ignore_patterns: Vec<String>,
    /// Maximum entries to return. Zero means no explicit limit.
    pub max_entries: usize,
}

/// Directory list result.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileListResult {
    /// Provider-scoped entries.
    pub entries: Vec<String>,
    /// Whether entries were omitted because of `max_entries`.
    pub truncated: bool,
    /// Total matching entries before truncation.
    pub total_entries: usize,
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
    /// Script interpreted by the provider's shell.
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

impl ShellCommand {
    /// Create a shell-script request.
    #[must_use]
    pub fn shell(script: impl Into<String>) -> Self {
        Self {
            command: script.into(),
            ..Self::default()
        }
    }
}

/// Structured direct-program execution request.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProgramCommand {
    /// Executable name or path passed directly to the operating system.
    pub program: String,
    /// Explicit argv entries. They are never interpreted by a shell.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub arguments: Vec<String>,
    /// Optional timeout in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    /// Optional provider-scoped working directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Environment variables to set for the program.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub environment: BTreeMap<String, String>,
}

impl ProgramCommand {
    /// Create a structured direct-program request.
    #[must_use]
    pub fn new<I, S>(program: impl Into<String>, arguments: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            program: program.into(),
            arguments: arguments.into_iter().map(Into::into).collect(),
            ..Self::default()
        }
    }

    /// Return a human-readable command representation for process snapshots.
    #[must_use]
    pub fn display_command(&self) -> String {
        std::iter::once(self.program.as_str())
            .chain(self.arguments.iter().map(String::as_str))
            .map(|part| format!("{part:?}"))
            .collect::<Vec<_>>()
            .join(" ")
    }
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

/// Product-neutral environment lifecycle category.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentLifecycleCategory {
    /// Environment is scoped to a durable session.
    Session,
    /// Environment is scoped to one run.
    Run,
    /// Environment is temporary and not durable by itself.
    Ephemeral,
}

/// Product-neutral environment lifecycle state.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentLifecycleState {
    /// Environment has been requested but no preparation has started.
    Pending,
    /// Environment preparation is in progress.
    Preparing,
    /// Environment is prepared and ready to serve operations.
    #[default]
    Ready,
    /// Environment is actively serving an operation.
    Running,
    /// Environment is prepared but idle.
    Idle,
    /// Environment was stopped by a lifecycle control.
    Stopped,
    /// Environment preparation or operation failed.
    Failed,
}

/// Explicit lifecycle controls supported by an environment provider.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EnvironmentLifecycleCapabilities {
    /// Lifecycle inspection is supported.
    pub inspect: bool,
    /// Explicit preparation is supported or safely handled as a no-op.
    pub prepare: bool,
    /// Explicit stop is supported.
    pub stop: bool,
    /// Explicit idle cleanup is supported.
    pub cleanup_idle: bool,
}

impl EnvironmentLifecycleCapabilities {
    /// Capabilities for providers that are already ready and require no
    /// explicit lifecycle management.
    #[must_use]
    pub const fn passive_ready() -> Self {
        Self {
            inspect: true,
            prepare: true,
            stop: false,
            cleanup_idle: false,
        }
    }
}

impl Default for EnvironmentLifecycleCapabilities {
    fn default() -> Self {
        Self::passive_ready()
    }
}

/// Serializable, product-neutral environment lifecycle snapshot.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EnvironmentLifecycleSnapshot {
    /// SDK provider identifier.
    pub provider_id: String,
    /// Optional provider-specific reusable environment id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_id: Option<String>,
    /// Optional lifecycle category.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<EnvironmentLifecycleCategory>,
    /// Neutral lifecycle state.
    pub state: EnvironmentLifecycleState,
    /// Explicit lifecycle controls supported by this provider.
    pub capabilities: EnvironmentLifecycleCapabilities,
    /// Provider metadata safe to store with session evidence.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

impl EnvironmentLifecycleSnapshot {
    /// Build a ready lifecycle snapshot for a provider that does not need
    /// explicit lifecycle control.
    #[must_use]
    pub fn ready(provider_id: impl Into<String>) -> Self {
        Self {
            provider_id: provider_id.into(),
            environment_id: None,
            category: None,
            state: EnvironmentLifecycleState::Ready,
            capabilities: EnvironmentLifecycleCapabilities::passive_ready(),
            metadata: Metadata::default(),
        }
    }

    /// Attach a provider-specific reusable environment id.
    #[must_use]
    pub fn with_environment_id(mut self, environment_id: impl Into<String>) -> Self {
        self.environment_id = Some(environment_id.into());
        self
    }

    /// Attach a lifecycle category.
    #[must_use]
    pub const fn with_category(mut self, category: EnvironmentLifecycleCategory) -> Self {
        self.category = Some(category);
        self
    }

    /// Replace lifecycle state.
    #[must_use]
    pub const fn with_state(mut self, state: EnvironmentLifecycleState) -> Self {
        self.state = state;
        self
    }

    /// Replace lifecycle capabilities.
    #[must_use]
    pub const fn with_capabilities(
        mut self,
        capabilities: EnvironmentLifecycleCapabilities,
    ) -> Self {
        self.capabilities = capabilities;
        self
    }

    /// Attach metadata.
    #[must_use]
    pub fn with_metadata(mut self, metadata: Metadata) -> Self {
        self.metadata = metadata;
        self
    }
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
    /// Background process snapshots.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub processes: Vec<ShellProcessSnapshot>,
    /// Provider metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

impl VersionedRecord for EnvironmentState {
    const SCHEMA: &'static str = "starweaver.environment.state";
    const ALLOW_BARE_V0: bool = true;
}

impl EnvironmentState {
    /// Convert the snapshot into a versioned JSON envelope for durable state domains.
    #[must_use]
    pub fn to_json(&self) -> Value {
        to_versioned_value(self).unwrap_or_else(|_| serde_json::json!({}))
    }

    /// Decode a current envelope or an explicitly supported previous-release bare snapshot.
    ///
    /// # Errors
    ///
    /// Returns a compatibility error for malformed, mismatched, or unknown versions.
    pub fn from_json(value: Value) -> Result<Self, VersionedRecordError> {
        from_versioned_value(value)
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
