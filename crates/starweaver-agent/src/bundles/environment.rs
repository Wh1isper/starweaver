use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use starweaver_context::AgentContext;
use starweaver_core::Metadata;
use starweaver_environment::{DynEnvironmentProvider, FileGlobOptions, FileGrepOptions};
use starweaver_tools::{
    DynToolset, EmptyToolArgs, StaticToolset, ToolContext, ToolError, ToolInstruction, ToolResult,
};

use super::helpers::{static_tool, static_tool_with_metadata, tool_execution_error};

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
    let mut approval_metadata = Metadata::default();
    approval_metadata.insert("approval_required".to_string(), serde_json::json!(true));

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

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct FilePathArgs {
    #[serde(alias = "path")]
    file_path: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct ListArgs {
    #[serde(alias = "path")]
    #[serde(default)]
    root: String,
    ignore: Option<Vec<String>>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct WriteArgs {
    #[serde(alias = "path")]
    file_path: String,
    content: String,
    mode: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct EditArgs {
    file_path: String,
    old_string: String,
    new_string: String,
    #[serde(default)]
    replace_all: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct EditItemArgs {
    old_string: String,
    new_string: String,
    #[serde(default)]
    replace_all: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct MultiEditArgs {
    file_path: String,
    edits: Vec<EditItemArgs>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct GlobArgs {
    pattern: String,
    #[serde(alias = "path")]
    #[serde(default)]
    root: String,
    #[serde(default)]
    include_hidden: bool,
    #[serde(default)]
    include_ignored: bool,
    max_results: Option<usize>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct GrepArgs {
    pattern: String,
    include: Option<String>,
    #[serde(alias = "path")]
    #[serde(default)]
    root: String,
    context_lines: Option<usize>,
    max_results: Option<usize>,
    max_matches_per_file: Option<usize>,
    #[serde(default)]
    include_hidden: bool,
    #[serde(default)]
    include_ignored: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct BatchPathsArgs {
    paths: Vec<String>,
    #[serde(default)]
    parents: bool,
    #[serde(default)]
    recursive: bool,
    #[serde(default)]
    force: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct PathPairArgs {
    src: String,
    dst: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct PathPairsArgs {
    operations: Vec<PathPairArgs>,
    #[serde(default)]
    overwrite: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct ShellExecArgs {
    command: String,
    #[serde(default)]
    background: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct ProcessIdArgs {
    process_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct ShellWaitArgs {
    process_id: String,
    timeout_seconds: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct ShellInputArgs {
    process_id: String,
    text: String,
    #[serde(default)]
    close_stdin: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct ShellSignalArgs {
    process_id: String,
    signal: i32,
}

async fn read_text(
    tool_context: ToolContext,
    arguments: FilePathArgs,
) -> Result<ToolResult, ToolError> {
    let provider = environment_provider(&tool_context, "view")?;
    let content = provider
        .read_text(&arguments.file_path)
        .await
        .map_err(|error| tool_execution_error("view", error))?;
    Ok(ToolResult::new(
        serde_json::json!({"file_path": arguments.file_path, "content": content}),
    ))
}

async fn list_files(context: ToolContext, arguments: ListArgs) -> Result<ToolResult, ToolError> {
    let provider = environment_provider(&context, "ls")?;
    let entries = provider
        .list(&arguments.root)
        .await
        .map_err(|error| tool_execution_error("ls", error))?;
    Ok(ToolResult::new(serde_json::json!({
        "root": arguments.root,
        "ignore": arguments.ignore.unwrap_or_default(),
        "entries": entries,
    })))
}

async fn write_text(
    tool_context: ToolContext,
    arguments: WriteArgs,
) -> Result<ToolResult, ToolError> {
    let provider = environment_provider(&tool_context, "write")?;
    let next_content = if arguments.mode.as_deref() == Some("a") {
        match provider.read_text(&arguments.file_path).await {
            Ok(existing) => format!("{existing}{}", arguments.content),
            Err(_) => arguments.content,
        }
    } else {
        arguments.content
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
                max_results: arguments.max_results.unwrap_or(500),
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
                include: arguments.include,
                context_lines: arguments.context_lines.unwrap_or(0),
                max_results: arguments.max_results.unwrap_or(100),
                max_matches_per_file: arguments.max_matches_per_file.unwrap_or(20),
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

async fn mkdir_paths(
    _context: ToolContext,
    arguments: BatchPathsArgs,
) -> Result<ToolResult, ToolError> {
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
    arguments: BatchPathsArgs,
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
            "operations": arguments.operations,
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
            "operations": arguments.operations,
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
        return Ok(ToolResult::new(serde_json::json!({
            "process_id": format!("{}_bg_{}", provider.id(), context.run_step),
            "command": arguments.command,
            "status": "pending",
            "provider_id": provider.id(),
        })));
    }
    let output = provider
        .run_shell(&arguments.command)
        .await
        .map_err(|error| tool_execution_error("shell_exec", error))?;
    Ok(ToolResult::new(serde_json::json!({
        "command": arguments.command,
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

fn operation(name: &str, payload: Value) -> ToolResult {
    let mut content = serde_json::Map::new();
    content.insert("operation".to_string(), Value::String(name.to_string()));
    content.insert("payload".to_string(), payload);
    ToolResult::new(Value::Object(content))
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
