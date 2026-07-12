//! `EnvD` request and result DTOs.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use starweaver_core::{Metadata, ProtocolIdentity};

use crate::{FileReadMode, ProcessSnapshot};

/// Initialize request.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct InitializeEnvdRequest {
    /// Protocol identity requested by the caller. Omitted only by legacy clients.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<ProtocolIdentity>,
    /// Caller name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_name: Option<String>,
    /// Caller metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

/// Initialize result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct InitializeEnvdResult {
    /// Negotiated protocol identity.
    pub protocol: ProtocolIdentity,
    /// Service name.
    pub service_name: String,
    /// Service metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

/// Open environment request.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct OpenEnvironmentRequest {
    /// Requested environment id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_id: Option<String>,
    /// Open metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

/// Environment-scoped request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EnvironmentRequest {
    /// Environment id.
    pub environment_id: String,
}

/// Idle lifecycle cleanup request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CleanupIdleRequest {
    /// Environment id.
    pub environment_id: String,
    /// Optional minimum idle age in seconds before cleanup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub older_than_seconds: Option<u64>,
}

/// File read request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileReadRequest {
    /// Environment id.
    pub environment_id: String,
    /// Environment path.
    pub path: String,
    /// Byte offset.
    pub offset: usize,
    /// Optional byte length.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<usize>,
    /// Read mode.
    pub mode: FileReadMode,
}

/// File read result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileReadResult {
    /// File bytes.
    pub bytes: Vec<u8>,
}

/// File write request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileWriteRequest {
    /// Environment id.
    pub environment_id: String,
    /// Environment path.
    pub path: String,
    /// File bytes.
    pub bytes: Vec<u8>,
}

/// File write result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileWriteResult {
    /// State version after the write.
    pub state_version: u64,
    /// Operation id for the write.
    pub operation_id: String,
}

/// Directory create request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileCreateDirRequest {
    /// Environment id.
    pub environment_id: String,
    /// Directory path.
    pub path: String,
    /// Whether parents should be created.
    pub parents: bool,
}

/// Path delete request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileDeleteRequest {
    /// Environment id.
    pub environment_id: String,
    /// Path to delete.
    pub path: String,
    /// Whether directory deletion is recursive.
    pub recursive: bool,
}

/// Path move request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileMoveRequest {
    /// Environment id.
    pub environment_id: String,
    /// Source path.
    pub src: String,
    /// Destination path.
    pub dst: String,
    /// Whether destination can be overwritten.
    pub overwrite: bool,
}

/// Path copy request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileCopyRequest {
    /// Environment id.
    pub environment_id: String,
    /// Source path.
    pub src: String,
    /// Destination path.
    pub dst: String,
    /// Whether destination can be overwritten.
    pub overwrite: bool,
}

/// Temporary file write request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileWriteTmpRequest {
    /// Environment id.
    pub environment_id: String,
    /// Requested filename.
    pub filename: String,
    /// File bytes.
    pub bytes: Vec<u8>,
}

/// Temporary file write result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileWriteTmpResult {
    /// Provider-visible temporary path.
    pub path: String,
    /// State version after the write.
    pub state_version: u64,
    /// Operation id for the write.
    pub operation_id: String,
}

/// Generic mutation result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MutationResult {
    /// State version after the mutation.
    pub state_version: u64,
    /// Operation id.
    pub operation_id: String,
}

/// File stat request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileStatRequest {
    /// Environment id.
    pub environment_id: String,
    /// Path to stat.
    pub path: String,
}

/// File metadata.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileStat {
    /// Size in bytes.
    pub size: u64,
    /// Whether the path is a regular file.
    pub is_file: bool,
    /// Whether the path is a directory.
    pub is_dir: bool,
    /// Modified timestamp in Unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified_unix_seconds: Option<u64>,
}

/// Directory list options.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileListOptions {
    /// Entry ignore patterns.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ignore_patterns: Vec<String>,
    /// Maximum entries. Zero means unlimited.
    pub max_entries: usize,
}

/// Directory list request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileListRequest {
    /// Environment id.
    pub environment_id: String,
    /// Path to list.
    pub path: String,
    /// List options.
    pub options: FileListOptions,
}

/// Directory list result.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileListResult {
    /// Entry paths.
    pub entries: Vec<String>,
    /// Whether results were truncated.
    pub truncated: bool,
    /// Total matching entries.
    pub total_entries: usize,
}

/// Glob options.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileGlobOptions {
    /// Include hidden files.
    pub include_hidden: bool,
    /// Include ignored files.
    pub include_ignored: bool,
    /// Maximum results. Zero means unlimited.
    pub max_results: usize,
}

/// Glob request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileGlobRequest {
    /// Environment id.
    pub environment_id: String,
    /// Root path.
    pub path: String,
    /// Glob pattern.
    pub pattern: String,
    /// Glob options.
    pub options: FileGlobOptions,
}

/// Glob match.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileGlobMatch {
    /// Matching path.
    pub path: String,
}

/// Grep options.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileGrepOptions {
    /// Optional include glob.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include: Option<String>,
    /// Context lines.
    pub context_lines: usize,
    /// Maximum results. Zero means unlimited.
    pub max_results: usize,
    /// Maximum matches per file. Zero means unlimited.
    pub max_matches_per_file: usize,
    /// Maximum files. Zero means unlimited.
    pub max_files: usize,
    /// Include hidden files.
    pub include_hidden: bool,
    /// Include ignored files.
    pub include_ignored: bool,
}

/// Grep request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileGrepRequest {
    /// Environment id.
    pub environment_id: String,
    /// Root path.
    pub path: String,
    /// Regex pattern.
    pub pattern: String,
    /// Grep options.
    pub options: FileGrepOptions,
}

/// Grep match.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileGrepMatch {
    /// Matching path.
    pub path: String,
    /// One-based line number.
    pub line_number: usize,
    /// Matching line.
    pub matching_line: String,
    /// Context block.
    pub context: String,
    /// One-based context start line.
    pub context_start_line: usize,
}

/// Foreground command request.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct CommandRunRequest {
    /// Environment id.
    pub environment_id: String,
    /// Command string.
    pub command: String,
    /// Timeout in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    /// Working directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Environment variables.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub environment: BTreeMap<String, String>,
}

/// Foreground command result.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct CommandRunResult {
    /// Exit status.
    pub status: i32,
    /// Captured stdout.
    pub stdout: String,
    /// Captured stderr.
    pub stderr: String,
    /// Command metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
    /// State version after recording the operation.
    pub state_version: u64,
    /// Operation id.
    pub operation_id: String,
}

/// Process start request.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProcessStartRequest {
    /// Environment id.
    pub environment_id: String,
    /// Command string.
    pub command: String,
    /// Timeout in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    /// Working directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Environment variables.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub environment: BTreeMap<String, String>,
}

/// Process wait request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProcessWaitRequest {
    /// Environment id.
    pub environment_id: String,
    /// Process id.
    pub process_id: String,
    /// Timeout in seconds.
    pub timeout_seconds: u64,
}

/// Process input request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProcessInputRequest {
    /// Environment id.
    pub environment_id: String,
    /// Process id.
    pub process_id: String,
    /// Text input.
    pub text: String,
    /// Whether stdin should be closed.
    pub close_stdin: bool,
}

/// Process signal request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProcessSignalRequest {
    /// Environment id.
    pub environment_id: String,
    /// Process id.
    pub process_id: String,
    /// Signal number.
    pub signal: i32,
}

/// Process kill request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProcessKillRequest {
    /// Environment id.
    pub environment_id: String,
    /// Process id.
    pub process_id: String,
}

/// Process list result.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProcessListResult {
    /// Process snapshots.
    pub processes: Vec<ProcessSnapshot>,
}

/// Environment context request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EnvironmentContextRequest {
    /// Environment id.
    pub environment_id: String,
}

/// Environment context result.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct EnvironmentContextResult {
    /// Optional model-facing context summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// Shell review context request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ShellReviewContextRequest {
    /// Environment id.
    pub environment_id: String,
}

/// Shell review context result.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ShellReviewContextResult {
    /// Default command working directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_cwd: Option<String>,
    /// Allowed path summaries.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_paths: Vec<String>,
    /// Shell platform.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell_platform: Option<String>,
    /// Shell executable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell_executable: Option<String>,
}
