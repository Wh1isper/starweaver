//! Shared request, response, search, shell, and state types for environment providers.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use starweaver_core::Metadata;

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
