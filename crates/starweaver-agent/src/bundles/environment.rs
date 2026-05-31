use std::{collections::BTreeMap, sync::Arc};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use starweaver_context::AgentContext;
use starweaver_environment::{
    DynEnvironmentProvider, EnvironmentError, EnvironmentProvider, FileGlobOptions, FileGrepOptions,
};
use starweaver_tools::{
    DynToolset, EmptyToolArgs, StaticToolset, ToolContext, ToolError, ToolInstruction, ToolResult,
};

use super::helpers::{static_tool, static_tool_with_metadata, tool_execution_error, tool_metadata};

/// `AgentContext` dependency that exposes the active SDK environment.
#[derive(Clone)]
pub struct EnvironmentHandle {
    provider: DynEnvironmentProvider,
}

impl EnvironmentHandle {
    /// Create an environment handle from a provider.
    #[must_use]
    pub fn new(provider: DynEnvironmentProvider) -> Self {
        Self { provider }
    }

    /// Return the underlying provider.
    #[must_use]
    pub fn provider(&self) -> DynEnvironmentProvider {
        self.provider.clone()
    }
}

/// Attach the active environment to an `AgentContext`.
pub fn attach_environment(context: &mut AgentContext, provider: DynEnvironmentProvider) {
    context
        .dependencies
        .insert(EnvironmentHandle::new(provider));
}

/// Create filesystem tools backed by the `EnvironmentHandle` stored in `AgentContext`.
#[must_use]
pub fn filesystem_tools() -> DynToolset {
    Arc::new(
        StaticToolset::new("filesystem")
            .with_id("filesystem")
            .with_instruction(ToolInstruction::new(
                "filesystem",
                "Filesystem tools operate inside the active AgentContext environment. Prefer glob to discover candidate paths, grep to find matching text, view for focused reads, write for intentional writes, edit or multi_edit for precise replacements, and resource_ref when a durable provider-scoped reference is enough.",
            ))
            .with_tools([
                static_tool("view", "Read a provider-scoped UTF-8 text file.", read_text),
                static_tool("ls", "List provider-scoped file entries.", list_files),
                static_tool("write", "Write a provider-scoped UTF-8 text file.", write_text),
                static_tool("edit", "Perform exact string replacement in files.", edit_text),
                static_tool("multi_edit", "Perform multiple exact replacements in one file.", multi_edit_text),
                static_tool("glob", "Find provider-scoped paths with ripgrep-style glob semantics.", glob_files),
                static_tool("grep", "Search provider-scoped text files with ripgrep regex semantics.", grep_files),
                static_tool("mkdir", "Create directories through a host/provider operation envelope.", mkdir_paths),
                static_tool("delete", "Delete files or directories through a host/provider operation envelope.", delete_paths),
                static_tool("move", "Move files or directories through a host/provider operation envelope.", move_paths),
                static_tool("copy", "Copy files through a host/provider operation envelope.", copy_paths),
                static_tool("resource_ref", "Create a stable provider-scoped resource reference for a path.", resource_ref),
            ]),
    )
}

/// Create shell tools backed by the `EnvironmentHandle` stored in `AgentContext`.
#[must_use]
pub fn shell_tools() -> DynToolset {
    let approval_metadata = tool_metadata("shell", false, true);

    Arc::new(
        StaticToolset::new("shell")
            .with_id("shell")
            .with_instruction(ToolInstruction::new(
                "shell",
                "Shell tools execute through the active AgentContext environment policy. Use shell_exec for bounded one-shot commands, set background=true for long-running work, and use shell_wait, shell_status, shell_input, shell_signal, or shell_kill for durable background handles when the provider supports them.",
            ))
            .with_tools([
                static_tool_with_metadata(
                    "shell_exec",
                    "Run a provider-scoped shell command. Set background=true for a durable background handle.",
                    approval_metadata,
                    shell_exec,
                ),
                static_tool("shell_wait", "Wait for or poll a background shell process.", shell_wait),
                static_tool("shell_status", "List background shell process status.", shell_status),
                static_tool("shell_input", "Write text to a background process stdin.", shell_input),
                static_tool("shell_signal", "Send a Unix signal to a background process.", shell_signal),
                static_tool("shell_kill", "Terminate and clean up a background shell process.", shell_kill),
            ]),
    )
}

const fn default_view_line_limit() -> usize {
    300
}

const fn default_view_max_line_length() -> usize {
    2000
}

fn default_root() -> String {
    ".".to_string()
}

fn default_grep_include() -> String {
    "**/*".to_string()
}

const fn default_glob_max_results() -> isize {
    500
}

const fn default_grep_context_lines() -> isize {
    2
}

const fn default_grep_max_results() -> isize {
    100
}

const fn default_grep_max_matches_per_file() -> isize {
    20
}

const fn default_grep_max_files() -> isize {
    50
}

const fn default_shell_timeout_seconds() -> u64 {
    180
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct FilePathArgs {
    /// Relative path to the file or resource.
    #[serde(alias = "path")]
    file_path: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct ViewArgs {
    /// Relative path to the file to read.
    #[serde(alias = "path")]
    file_path: String,
    /// Line number to start reading from (0-indexed).
    #[serde(default)]
    line_offset: Option<usize>,
    /// Maximum number of lines to read.
    #[serde(default = "default_view_line_limit")]
    line_limit: usize,
    /// Maximum length of each line before truncation.
    #[serde(default = "default_view_max_line_length")]
    max_line_length: usize,
    /// Optional analysis instructions for image, video, or audio files.
    #[serde(default)]
    instructions: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct ListArgs {
    /// Directory relative path.
    #[serde(alias = "root", default = "default_root")]
    path: String,
    /// Glob patterns to ignore.
    #[serde(default)]
    ignore: Option<Vec<String>>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct WriteArgs {
    /// Relative path to the file to write.
    #[serde(alias = "path")]
    file_path: String,
    /// Content to write to the file.
    content: String,
    /// 'w' for write/overwrite, 'a' for append.
    #[serde(default)]
    mode: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct EditArgs {
    /// Relative path to the file to edit.
    file_path: String,
    /// Text to replace. Empty string creates a new file.
    old_string: String,
    /// New text to replace the old text with.
    new_string: String,
    /// Replace all occurrences.
    #[serde(default)]
    replace_all: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct EditItemArgs {
    /// Text to replace. Empty string is only allowed for the first create operation.
    old_string: String,
    /// New text to replace the old text with.
    new_string: String,
    /// Replace all occurrences.
    #[serde(default)]
    replace_all: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct MultiEditArgs {
    /// Relative path to the file to edit.
    file_path: String,
    /// Array of edit operations to perform in sequence.
    edits: Vec<EditItemArgs>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct GlobArgs {
    /// Ripgrep-style glob pattern to match files and directories.
    pattern: String,
    /// Logical root to search from.
    #[serde(alias = "path", default = "default_root")]
    root: String,
    /// Include hidden dot paths such as .git, .venv, and .env.
    #[serde(default)]
    include_hidden: bool,
    /// Include files ignored by .gitignore and nested ignore files.
    #[serde(default)]
    include_ignored: bool,
    /// Maximum number of results to return. Use -1 for unlimited.
    #[serde(default = "default_glob_max_results")]
    max_results: isize,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct GrepArgs {
    /// Ripgrep-style regular expression pattern to search for.
    pattern: String,
    /// Ripgrep-style glob pattern used to select files.
    #[serde(default = "default_grep_include")]
    include: String,
    /// Logical root to search from.
    #[serde(alias = "path", default = "default_root")]
    root: String,
    /// Context lines before and after matches.
    #[serde(default = "default_grep_context_lines")]
    context_lines: isize,
    /// Maximum total matches. Use -1 for unlimited.
    #[serde(default = "default_grep_max_results")]
    max_results: isize,
    /// Maximum matches per file. Use -1 for unlimited.
    #[serde(default = "default_grep_max_matches_per_file")]
    max_matches_per_file: isize,
    /// Maximum files to search. Use -1 for unlimited.
    #[serde(default = "default_grep_max_files")]
    max_files: isize,
    /// Include hidden dot paths such as .git, .venv, and .env.
    #[serde(default)]
    include_hidden: bool,
    /// Include files ignored by .gitignore and nested ignore files.
    #[serde(default)]
    include_ignored: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct MkdirArgs {
    /// List of directory paths to create.
    paths: Vec<String>,
    /// Create intermediate directories as needed.
    #[serde(default)]
    parents: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct DeleteArgs {
    /// List of file or directory paths to delete.
    paths: Vec<String>,
    /// Delete directories and their contents recursively.
    #[serde(default)]
    recursive: bool,
    /// Ignore missing paths.
    #[serde(default)]
    force: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct PathPairArgs {
    /// Source path.
    src: String,
    /// Destination path.
    dst: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct PathPairsArgs {
    /// List of {src, dst} pairs.
    #[serde(alias = "operations")]
    pairs: Vec<PathPairArgs>,
    /// Allow overwriting existing destinations.
    #[serde(default)]
    overwrite: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct ShellExecArgs {
    /// The shell command to execute.
    command: String,
    /// Maximum execution time in seconds.
    #[serde(default = "default_shell_timeout_seconds")]
    timeout_seconds: u64,
    /// Environment variables to set for the command.
    #[serde(default)]
    environment: Option<BTreeMap<String, String>>,
    /// Working directory relative or absolute path.
    #[serde(default)]
    cwd: Option<String>,
    /// Run command in background and return a process handle when supported.
    #[serde(default)]
    background: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct ProcessIdArgs {
    /// Process ID of the background process.
    process_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct ShellWaitArgs {
    /// Process ID returned by shell with background=true.
    process_id: String,
    /// Maximum seconds to wait. 0 means poll.
    #[serde(default = "default_shell_timeout_seconds")]
    timeout_seconds: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct ShellInputArgs {
    /// Process ID of the background process.
    process_id: String,
    /// Text to write to stdin. A trailing newline is added automatically.
    text: String,
    /// Close stdin after writing.
    #[serde(default)]
    close_stdin: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct ShellSignalArgs {
    /// Process ID of the background process.
    process_id: String,
    /// Signal number to send.
    signal: i32,
}

async fn read_text(
    tool_context: ToolContext,
    arguments: ViewArgs,
) -> Result<ToolResult, ToolError> {
    let provider = environment_provider(&tool_context, "view")?;
    let content = provider
        .read_text(&arguments.file_path)
        .await
        .map_err(|error| tool_execution_error("view", error))?;
    let selected = select_text_lines(
        &content,
        arguments.line_offset.unwrap_or(0),
        arguments.line_limit,
        arguments.max_line_length,
    );
    Ok(ToolResult::new(serde_json::json!({
        "file_path": arguments.file_path,
        "content": selected.content,
        "line_offset": arguments.line_offset,
        "line_limit": arguments.line_limit,
        "max_line_length": arguments.max_line_length,
        "instructions": arguments.instructions,
        "truncated": selected.truncated,
    })))
}

async fn list_files(context: ToolContext, arguments: ListArgs) -> Result<ToolResult, ToolError> {
    let provider = environment_provider(&context, "ls")?;
    let ignore = arguments.ignore.unwrap_or_default();
    let entries = provider
        .list(&arguments.path)
        .await
        .map_err(|error| tool_execution_error("ls", error))?
        .into_iter()
        .filter(|entry| !ignore.iter().any(|pattern| ignore_match(pattern, entry)))
        .collect::<Vec<_>>();
    Ok(ToolResult::new(serde_json::json!({
        "path": arguments.path,
        "ignore": ignore,
        "entries": entries,
    })))
}

async fn write_text(
    tool_context: ToolContext,
    arguments: WriteArgs,
) -> Result<ToolResult, ToolError> {
    let provider = environment_provider(&tool_context, "write")?;
    let next_content = match arguments.mode.as_deref() {
        Some("a") => match provider.read_text(&arguments.file_path).await {
            Ok(existing) => format!("{existing}{}", arguments.content),
            Err(EnvironmentError::NotFound(_)) => arguments.content,
            Err(error) => return Err(tool_execution_error("write", error)),
        },
        Some("w") | None => arguments.content,
        Some(mode) => {
            return Err(tool_execution_error(
                "write",
                format!("unsupported write mode {mode:?}"),
            ));
        }
    };
    provider
        .write_text(&arguments.file_path, &next_content)
        .await
        .map_err(|error| tool_execution_error("write", error))?;
    Ok(ToolResult::new(
        serde_json::json!({"file_path": arguments.file_path, "written": true}),
    ))
}

async fn edit_text(
    tool_context: ToolContext,
    arguments: EditArgs,
) -> Result<ToolResult, ToolError> {
    let provider = environment_provider(&tool_context, "edit")?;
    if arguments.old_string.is_empty() {
        ensure_file_missing(provider.as_ref(), "edit", &arguments.file_path).await?;
        provider
            .write_text(&arguments.file_path, &arguments.new_string)
            .await
            .map_err(|error| tool_execution_error("edit", error))?;
        return Ok(ToolResult::new(serde_json::json!({
            "file_path": arguments.file_path,
            "created": true,
        })));
    }
    let file_content = provider
        .read_text(&arguments.file_path)
        .await
        .map_err(|error| tool_execution_error("edit", error))?;
    let updated = apply_replacement(
        "edit",
        &file_content,
        &arguments.old_string,
        &arguments.new_string,
        arguments.replace_all,
    )?;
    provider
        .write_text(&arguments.file_path, &updated)
        .await
        .map_err(|error| tool_execution_error("edit", error))?;
    Ok(ToolResult::new(serde_json::json!({
        "file_path": arguments.file_path,
        "edited": true,
    })))
}

async fn multi_edit_text(
    tool_context: ToolContext,
    arguments: MultiEditArgs,
) -> Result<ToolResult, ToolError> {
    let provider = environment_provider(&tool_context, "multi_edit")?;
    let mut edits = arguments.edits.into_iter();
    let Some(first) = edits.next() else {
        return Err(tool_execution_error(
            "multi_edit",
            "at least one edit is required",
        ));
    };
    let mut updated_content = if first.old_string.is_empty() {
        if edits.len() > 0 {
            return Err(tool_execution_error(
                "multi_edit",
                "create operation must be the only edit when old_string is empty",
            ));
        }
        ensure_file_missing(provider.as_ref(), "multi_edit", &arguments.file_path).await?;
        first.new_string
    } else {
        let existing = provider
            .read_text(&arguments.file_path)
            .await
            .map_err(|error| tool_execution_error("multi_edit", error))?;
        apply_replacement(
            "multi_edit",
            &existing,
            &first.old_string,
            &first.new_string,
            first.replace_all,
        )?
    };
    for edit in edits {
        updated_content = apply_replacement(
            "multi_edit",
            &updated_content,
            &edit.old_string,
            &edit.new_string,
            edit.replace_all,
        )?;
    }
    provider
        .write_text(&arguments.file_path, &updated_content)
        .await
        .map_err(|error| tool_execution_error("multi_edit", error))?;
    Ok(ToolResult::new(serde_json::json!({
        "file_path": arguments.file_path,
        "edited": true,
    })))
}

async fn glob_files(context: ToolContext, arguments: GlobArgs) -> Result<ToolResult, ToolError> {
    let provider = environment_provider(&context, "glob")?;
    let matches = provider
        .glob(
            &arguments.root,
            &arguments.pattern,
            FileGlobOptions {
                include_hidden: arguments.include_hidden,
                include_ignored: arguments.include_ignored,
                max_results: limit_or_unlimited("glob", "max_results", arguments.max_results)?,
            },
        )
        .await
        .map_err(|error| tool_execution_error("glob", error))?;
    Ok(ToolResult::new(serde_json::json!({
        "root": arguments.root,
        "pattern": arguments.pattern,
        "matches": matches,
    })))
}

async fn grep_files(context: ToolContext, arguments: GrepArgs) -> Result<ToolResult, ToolError> {
    let provider = environment_provider(&context, "grep")?;
    let matches = provider
        .grep(
            &arguments.root,
            &arguments.pattern,
            FileGrepOptions {
                include: Some(arguments.include),
                context_lines: non_negative_limit(
                    "grep",
                    "context_lines",
                    arguments.context_lines,
                )?,
                max_results: limit_or_unlimited("grep", "max_results", arguments.max_results)?,
                max_matches_per_file: limit_or_unlimited(
                    "grep",
                    "max_matches_per_file",
                    arguments.max_matches_per_file,
                )?,
                max_files: limit_or_unlimited("grep", "max_files", arguments.max_files)?,
                include_hidden: arguments.include_hidden,
                include_ignored: arguments.include_ignored,
            },
        )
        .await
        .map_err(|error| tool_execution_error("grep", error))?;
    Ok(ToolResult::new(serde_json::json!({
        "root": arguments.root,
        "pattern": arguments.pattern,
        "matches": matches,
    })))
}

async fn mkdir_paths(_context: ToolContext, arguments: MkdirArgs) -> Result<ToolResult, ToolError> {
    Ok(operation(
        "mkdir",
        serde_json::json!({
            "paths": arguments.paths,
            "parents": arguments.parents,
        }),
    ))
}

async fn delete_paths(
    _context: ToolContext,
    arguments: DeleteArgs,
) -> Result<ToolResult, ToolError> {
    Ok(operation(
        "delete",
        serde_json::json!({
            "paths": arguments.paths,
            "recursive": arguments.recursive,
            "force": arguments.force,
        }),
    ))
}

async fn move_paths(
    _context: ToolContext,
    arguments: PathPairsArgs,
) -> Result<ToolResult, ToolError> {
    Ok(operation(
        "move",
        serde_json::json!({
            "pairs": arguments.pairs,
            "overwrite": arguments.overwrite,
        }),
    ))
}

async fn copy_paths(
    _context: ToolContext,
    arguments: PathPairsArgs,
) -> Result<ToolResult, ToolError> {
    Ok(operation(
        "copy",
        serde_json::json!({
            "pairs": arguments.pairs,
            "overwrite": arguments.overwrite,
        }),
    ))
}

async fn resource_ref(
    context: ToolContext,
    arguments: FilePathArgs,
) -> Result<ToolResult, ToolError> {
    let provider = environment_provider(&context, "resource_ref")?;
    Ok(ToolResult::new(serde_json::json!({
        "id": format!("{}:{}", provider.id(), arguments.file_path),
        "uri": format!("env://{}/{}", provider.id(), arguments.file_path),
        "metadata": {"provider_id": provider.id(), "file_path": arguments.file_path},
    })))
}

async fn shell_exec(
    context: ToolContext,
    arguments: ShellExecArgs,
) -> Result<ToolResult, ToolError> {
    let provider = environment_provider(&context, "shell_exec")?;
    if arguments.background {
        return Err(tool_execution_error(
            "shell_exec",
            "background shell execution requires a durable shell provider",
        ));
    }
    let output = provider
        .run_shell(&arguments.command)
        .await
        .map_err(|error| tool_execution_error("shell_exec", error))?;
    Ok(ToolResult::new(serde_json::json!({
        "command": arguments.command,
        "timeout_seconds": arguments.timeout_seconds,
        "environment": arguments.environment.unwrap_or_default(),
        "cwd": arguments.cwd,
        "return_code": output.status,
        "stdout": output.stdout,
        "stderr": output.stderr,
    })))
}

async fn shell_wait(
    _context: ToolContext,
    arguments: ShellWaitArgs,
) -> Result<ToolResult, ToolError> {
    Ok(operation(
        "shell_wait",
        serde_json::json!({
            "process_id": arguments.process_id,
            "timeout_seconds": arguments.timeout_seconds,
        }),
    ))
}

async fn shell_status(
    _context: ToolContext,
    _arguments: EmptyToolArgs,
) -> Result<ToolResult, ToolError> {
    Ok(operation("shell_status", serde_json::json!({})))
}

async fn shell_input(
    _context: ToolContext,
    arguments: ShellInputArgs,
) -> Result<ToolResult, ToolError> {
    Ok(operation(
        "shell_input",
        serde_json::json!({
            "process_id": arguments.process_id,
            "text": arguments.text,
            "close_stdin": arguments.close_stdin,
        }),
    ))
}

async fn shell_signal(
    _context: ToolContext,
    arguments: ShellSignalArgs,
) -> Result<ToolResult, ToolError> {
    Ok(operation(
        "shell_signal",
        serde_json::json!({
            "process_id": arguments.process_id,
            "signal": arguments.signal,
        }),
    ))
}

async fn shell_kill(
    _context: ToolContext,
    arguments: ProcessIdArgs,
) -> Result<ToolResult, ToolError> {
    Ok(operation(
        "shell_kill",
        serde_json::json!({"process_id": arguments.process_id}),
    ))
}

fn apply_replacement(
    tool: &str,
    content: &str,
    old_string: &str,
    new_string: &str,
    replace_all: bool,
) -> Result<String, ToolError> {
    if old_string.is_empty() {
        return Err(tool_execution_error(
            tool,
            "old_string must be non-empty for replacement",
        ));
    }
    if !content.contains(old_string) {
        return Err(tool_execution_error(tool, "text not found"));
    }
    if replace_all {
        return Ok(content.replace(old_string, new_string));
    }
    let occurrences = content.matches(old_string).count();
    if occurrences > 1 {
        return Err(tool_execution_error(
            tool,
            "text appears multiple times; add context or use replace_all=true",
        ));
    }
    Ok(content.replacen(old_string, new_string, 1))
}

struct TextSelection {
    content: String,
    truncated: bool,
}

fn select_text_lines(
    content: &str,
    line_offset: usize,
    line_limit: usize,
    max_line_length: usize,
) -> TextSelection {
    let lines = content.lines().skip(line_offset).take(line_limit);
    let mut truncated = content.lines().count() > line_offset.saturating_add(line_limit);
    let content = lines
        .map(|line| {
            if line.chars().count() > max_line_length {
                truncated = true;
                line.chars().take(max_line_length).collect::<String>()
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    TextSelection { content, truncated }
}

fn non_negative_limit(tool: &str, field: &str, value: isize) -> Result<usize, ToolError> {
    if value < 0 {
        return Err(tool_execution_error(
            tool,
            format!("{field} must be greater than or equal to 0"),
        ));
    }
    usize::try_from(value).map_err(|error| tool_execution_error(tool, error))
}

fn limit_or_unlimited(tool: &str, field: &str, value: isize) -> Result<usize, ToolError> {
    if value < -1 {
        return Err(tool_execution_error(
            tool,
            format!("{field} must be greater than or equal to -1"),
        ));
    }
    Ok(if value == -1 {
        0
    } else {
        usize::try_from(value).map_err(|error| tool_execution_error(tool, error))?
    })
}

async fn ensure_file_missing(
    provider: &dyn EnvironmentProvider,
    tool: &str,
    path: &str,
) -> Result<(), ToolError> {
    match provider.read_text(path).await {
        Ok(_) => Err(tool_execution_error(
            tool,
            "file already exists; use write to overwrite existing content",
        )),
        Err(EnvironmentError::NotFound(_)) => Ok(()),
        Err(error) => Err(tool_execution_error(tool, error)),
    }
}

fn operation(name: &str, payload: Value) -> ToolResult {
    let mut content = serde_json::Map::new();
    content.insert("operation".to_string(), Value::String(name.to_string()));
    content.insert("payload".to_string(), payload);
    ToolResult::new(Value::Object(content))
}

fn ignore_match(pattern: &str, entry: &str) -> bool {
    entry == pattern
        || entry.ends_with(pattern)
        || entry.contains(pattern)
        || pattern
            .strip_suffix('/')
            .is_some_and(|prefix| entry.starts_with(prefix))
}

fn environment_provider(
    context: &ToolContext,
    tool: &str,
) -> Result<DynEnvironmentProvider, ToolError> {
    let agent_context = context.dependency::<AgentContext>().ok_or_else(|| {
        tool_execution_error(tool, "AgentContext dependency is missing from ToolContext")
    })?;
    let environment = agent_context
        .dependencies
        .get::<EnvironmentHandle>()
        .ok_or_else(|| {
            tool_execution_error(tool, "EnvironmentHandle is missing from AgentContext")
        })?;
    Ok(environment.provider())
}
