use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub(super) const fn default_view_line_limit() -> usize {
    300
}

pub(super) const fn default_view_max_line_length() -> usize {
    2000
}

pub(super) fn default_root() -> String {
    ".".to_string()
}

pub(super) fn default_grep_include() -> String {
    "**/*".to_string()
}

pub(super) const fn default_glob_max_results() -> isize {
    500
}

pub(super) const fn default_grep_context_lines() -> isize {
    2
}

pub(super) const fn default_grep_max_results() -> isize {
    100
}

pub(super) const fn default_grep_max_matches_per_file() -> isize {
    20
}

pub(super) const fn default_grep_max_files() -> isize {
    50
}

pub(super) const fn default_shell_timeout_seconds() -> u64 {
    180
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub(super) struct FilePathArgs {
    /// Relative path to the file or resource.
    #[serde(alias = "path")]
    pub(super) file_path: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub(super) struct ViewArgs {
    /// Relative path to the file to read.
    #[serde(alias = "path")]
    pub(super) file_path: String,
    /// Line number to start reading from (0-indexed).
    #[serde(default)]
    pub(super) line_offset: Option<usize>,
    /// Maximum number of lines to read.
    #[serde(default = "default_view_line_limit")]
    pub(super) line_limit: usize,
    /// Maximum length of each line before truncation.
    #[serde(default = "default_view_max_line_length")]
    pub(super) max_line_length: usize,
    /// Optional analysis instructions for image, video, or audio files.
    #[serde(default)]
    pub(super) instructions: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub(super) struct ListArgs {
    /// Directory relative path.
    #[serde(alias = "root", default = "default_root")]
    pub(super) path: String,
    /// Glob patterns to ignore.
    #[serde(default)]
    pub(super) ignore: Option<Vec<String>>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub(super) struct WriteArgs {
    /// Relative path to the file to write.
    #[serde(alias = "path")]
    pub(super) file_path: String,
    /// Content to write to the file.
    pub(super) content: String,
    /// 'w' for write/overwrite, 'a' for append.
    #[serde(default)]
    pub(super) mode: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub(super) struct EditArgs {
    /// Relative path to the file to edit.
    pub(super) file_path: String,
    /// Text to replace. Empty string creates a new file.
    pub(super) old_string: String,
    /// New text to replace the old text with.
    pub(super) new_string: String,
    /// Replace all occurrences.
    #[serde(default)]
    pub(super) replace_all: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub(super) struct EditItemArgs {
    /// Text to replace. Empty string is only allowed for the first create operation.
    pub(super) old_string: String,
    /// New text to replace the old text with.
    pub(super) new_string: String,
    /// Replace all occurrences.
    #[serde(default)]
    pub(super) replace_all: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub(super) struct MultiEditArgs {
    /// Relative path to the file to edit.
    pub(super) file_path: String,
    /// Array of edit operations to perform in sequence.
    pub(super) edits: Vec<EditItemArgs>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub(super) struct GlobArgs {
    /// Ripgrep-style glob pattern to match files and directories.
    pub(super) pattern: String,
    /// Logical root to search from.
    #[serde(alias = "path", default = "default_root")]
    pub(super) root: String,
    /// Include hidden dot paths such as .git, .venv, and .env.
    #[serde(default)]
    pub(super) include_hidden: bool,
    /// Include files ignored by .gitignore and nested ignore files.
    #[serde(default)]
    pub(super) include_ignored: bool,
    /// Maximum number of results to return. Use -1 for unlimited.
    #[serde(default = "default_glob_max_results")]
    pub(super) max_results: isize,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub(super) struct GrepArgs {
    /// Ripgrep-style regular expression pattern to search for.
    pub(super) pattern: String,
    /// Ripgrep-style glob pattern used to select files.
    #[serde(default = "default_grep_include")]
    pub(super) include: String,
    /// Logical root to search from.
    #[serde(alias = "path", default = "default_root")]
    pub(super) root: String,
    /// Context lines before and after matches.
    #[serde(default = "default_grep_context_lines")]
    pub(super) context_lines: isize,
    /// Maximum total matches. Use -1 for unlimited.
    #[serde(default = "default_grep_max_results")]
    pub(super) max_results: isize,
    /// Maximum matches per file. Use -1 for unlimited.
    #[serde(default = "default_grep_max_matches_per_file")]
    pub(super) max_matches_per_file: isize,
    /// Maximum files to search. Use -1 for unlimited.
    #[serde(default = "default_grep_max_files")]
    pub(super) max_files: isize,
    /// Include hidden dot paths such as .git, .venv, and .env.
    #[serde(default)]
    pub(super) include_hidden: bool,
    /// Include files ignored by .gitignore and nested ignore files.
    #[serde(default)]
    pub(super) include_ignored: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub(super) struct MkdirArgs {
    /// List of directory paths to create.
    pub(super) paths: Vec<String>,
    /// Create intermediate directories as needed.
    #[serde(default)]
    pub(super) parents: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub(super) struct DeleteArgs {
    /// List of file or directory paths to delete.
    pub(super) paths: Vec<String>,
    /// Delete directories and their contents recursively.
    #[serde(default)]
    pub(super) recursive: bool,
    /// Ignore missing paths.
    #[serde(default)]
    pub(super) force: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub(super) struct PathPairArgs {
    /// Source path.
    pub(super) src: String,
    /// Destination path.
    pub(super) dst: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub(super) struct PathPairsArgs {
    /// List of {src, dst} pairs.
    #[serde(alias = "operations")]
    pub(super) pairs: Vec<PathPairArgs>,
    /// Allow overwriting existing destinations.
    #[serde(default)]
    pub(super) overwrite: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub(super) struct ShellExecArgs {
    /// The shell command to execute.
    pub(super) command: String,
    /// Maximum execution time in seconds.
    #[serde(default = "default_shell_timeout_seconds")]
    pub(super) timeout_seconds: u64,
    /// Environment variables to set for the command.
    #[serde(default)]
    pub(super) environment: Option<BTreeMap<String, String>>,
    /// Working directory relative or absolute path.
    #[serde(default)]
    pub(super) cwd: Option<String>,
    /// Run command in background and return a process handle when supported.
    #[serde(default)]
    pub(super) background: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub(super) struct ProcessIdArgs {
    /// Process ID of the background process.
    pub(super) process_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub(super) struct ShellWaitArgs {
    /// Process ID returned by shell with background=true.
    pub(super) process_id: String,
    /// Maximum seconds to wait. 0 means poll.
    #[serde(default = "default_shell_timeout_seconds")]
    pub(super) timeout_seconds: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub(super) struct ShellInputArgs {
    /// Process ID of the background process.
    pub(super) process_id: String,
    /// Text to write to stdin. A trailing newline is added automatically.
    pub(super) text: String,
    /// Close stdin after writing.
    #[serde(default)]
    pub(super) close_stdin: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub(super) struct ShellSignalArgs {
    /// Process ID of the background process.
    pub(super) process_id: String,
    /// Signal number to send.
    pub(super) signal: i32,
}
