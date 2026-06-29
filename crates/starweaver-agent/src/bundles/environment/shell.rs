use std::sync::Arc;

use starweaver_context::{AgentContext, ToolConfig};
use starweaver_environment::{
    DynProcessShellProvider, EnvironmentProvider, ShellCommand, ShellProcessSnapshot,
    ShellReviewEnvironmentContext,
};
use starweaver_tools::{
    DynToolset, EmptyToolArgs, StaticToolset, ToolContext, ToolError, ToolInstruction, ToolResult,
};
use uuid::Uuid;

use super::{
    args::{ProcessIdArgs, ShellExecArgs, ShellInputArgs, ShellSignalArgs, ShellWaitArgs},
    handle::{environment_provider, maybe_environment_provider},
    shell_review::{review_shell_command_or_block, ShellReviewContextSnapshot},
};
use crate::bundles::helpers::{
    static_tool, static_tool_with_metadata, tool_execution_error, tool_invalid_arguments,
    tool_metadata,
};
use crate::bundles::output::{
    append_guidance, dump_tool_output, fit_text_fields_to_limit, output_too_large_message,
    tool_output_size, write_tmp_output, DEFAULT_TOOL_OUTPUT_TRUNCATE_LIMIT,
};

/// `AgentContext` dependency for process-capable shell providers.
#[derive(Clone)]
pub struct ProcessShellHandle {
    provider: DynProcessShellProvider,
}

impl ProcessShellHandle {
    /// Create a process shell handle.
    #[must_use]
    pub fn new(provider: DynProcessShellProvider) -> Self {
        Self { provider }
    }

    /// Return the underlying provider.
    #[must_use]
    pub fn provider(&self) -> DynProcessShellProvider {
        self.provider.clone()
    }
}

/// Attach a process-capable shell provider to an `AgentContext`.
pub fn attach_process_shell(context: &mut AgentContext, provider: DynProcessShellProvider) {
    context
        .dependencies
        .insert(ProcessShellHandle::new(provider));
}

/// Create shell tools backed by the `EnvironmentHandle` stored in `AgentContext`.
#[must_use]
#[allow(clippy::needless_raw_string_hashes)]
pub fn shell_tools() -> DynToolset {
    let approval_metadata = tool_metadata("shell", false, true);

    Arc::new(
        StaticToolset::new("shell")
            .with_id("shell")
            .with_instruction(ToolInstruction::new(
                "shell",
                r#"<shell-tool>
Execute shell commands via `shell_exec`. Check the runtime `<shell-environment>` context for the active shell dialect. Local POSIX environments use `/bin/bash` by default when available; other environments may use the platform default shell or a sandbox-specific shell.

Parameters:
- command (required): The shell command string to execute.
- timeout_seconds: Maximum execution time in seconds.
- environment: Environment variables as key-value pairs.
- cwd: Working directory (relative or absolute path).
- background (default false): Run command in background and return process_id immediately.

Large outputs are saved to temporary files with paths in stdout_file_path/stderr_file_path. If the complete structured shell result is still too large, the full result is saved to output_file_path and a bounded preview is returned. When a shell command needs to create temporary files itself, write them under `$TMPDIR` rather than hard-coded `/tmp/...`; local providers map `$TMPDIR` to a provider-managed, session-scoped directory that file tools can read.

<background-mode>
Set background=true for long-running commands such as builds, servers, and test suites. Manage background processes with:
- shell_wait: Wait for or poll a background process. Use timeout_seconds=0 to poll and drain current output immediately.
- shell_input: Write to a background process stdin for prompts, REPL commands, or piped data.
- shell_signal: Send a Unix signal such as 2 (SIGINT) or 15 (SIGTERM).
- shell_kill: Terminate a running process and clean up tracking.
- shell_status: List all background processes and their status.

Completed background processes are automatically reported in context. Use shell_wait only when you need results before proceeding.
</background-mode>

Avoid:
- find/grep for searching when glob or grep tools can do the job.
- cat/head/tail/ls to read files when view and ls tools can do the job.
- cd command; use the cwd parameter instead.
</shell-tool>"#,
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

async fn shell_exec(
    context: ToolContext,
    arguments: ShellExecArgs,
) -> Result<ToolResult, ToolError> {
    if arguments.command.trim().is_empty() {
        return Err(tool_invalid_arguments(
            "shell_exec",
            "command must not be empty. Provide a shell command string, or skip shell_exec if there is no command to run.",
        ));
    }
    let environment = merged_shell_environment(&context, arguments.environment.clone());
    let shell_command = ShellCommand {
        command: arguments.command.clone(),
        timeout_seconds: Some(arguments.timeout_seconds),
        cwd: arguments.cwd.clone(),
        environment: environment.clone(),
    };
    if arguments.background {
        let provider = process_provider(&context, "shell_exec")?;
        if let Some(blocked) = review_shell_command_or_block(
            &context,
            &arguments.command,
            arguments.cwd.as_deref(),
            arguments.background,
            environment.keys().cloned().collect(),
            arguments.timeout_seconds,
            shell_review_context(provider.shell_review_context(), arguments.timeout_seconds),
        )
        .await?
        {
            return Ok(blocked);
        }
        let snapshot = provider
            .start_process(shell_command)
            .await
            .map_err(|error| tool_execution_error("shell_exec", error))?;
        return process_result(&context, &snapshot).await;
    }
    let provider = environment_provider(&context, "shell_exec")?;
    if let Some(blocked) = review_shell_command_or_block(
        &context,
        &arguments.command,
        arguments.cwd.as_deref(),
        arguments.background,
        environment.keys().cloned().collect(),
        arguments.timeout_seconds,
        shell_review_context(provider.shell_review_context(), arguments.timeout_seconds),
    )
    .await?
    {
        return Ok(blocked);
    }
    let output = provider
        .run_shell(shell_command)
        .await
        .map_err(|error| tool_execution_error("shell_exec", error))?;
    let truncate_limit = shell_output_truncate_limit(&context);
    let stdout =
        truncate_shell_output(provider.as_ref(), "stdout", &output.stdout, truncate_limit).await;
    let stderr =
        truncate_shell_output(provider.as_ref(), "stderr", &output.stderr, truncate_limit).await;
    let mut result = serde_json::json!({
        "command": arguments.command,
        "timeout_seconds": arguments.timeout_seconds,
        "environment": environment,
        "cwd": arguments.cwd,
        "return_code": output.status,
        "stdout": stdout.content,
        "stderr": stderr.content,
    });
    if let Some(path) = stdout.file_path {
        result["stdout_file_path"] = serde_json::json!(path);
    }
    if let Some(path) = stderr.file_path {
        result["stderr_file_path"] = serde_json::json!(path);
    }
    Ok(ToolResult::new(
        guard_shell_result(&context, Some(provider.as_ref()), result, "shell-exec").await,
    ))
}

fn merged_shell_environment(
    context: &ToolContext,
    per_call: Option<std::collections::BTreeMap<String, String>>,
) -> std::collections::BTreeMap<String, String> {
    let mut environment = context
        .dependency::<AgentContext>()
        .map_or_else(std::collections::BTreeMap::new, |agent_context| {
            agent_context.shell_env.clone()
        });
    if let Some(per_call) = per_call {
        environment.extend(per_call);
    }
    environment
}

async fn shell_wait(
    context: ToolContext,
    arguments: ShellWaitArgs,
) -> Result<ToolResult, ToolError> {
    let provider = process_provider(&context, "shell_wait")?;
    let snapshot = provider
        .wait_process(&arguments.process_id, arguments.timeout_seconds)
        .await
        .map_err(|error| tool_execution_error("shell_wait", error))?;
    process_result(&context, &snapshot).await
}

async fn shell_status(
    context: ToolContext,
    _arguments: EmptyToolArgs,
) -> Result<ToolResult, ToolError> {
    let provider = process_provider(&context, "shell_status")?;
    let processes = provider
        .list_processes()
        .await
        .map_err(|error| tool_execution_error("shell_status", error))?;
    Ok(ToolResult::new(
        serde_json::json!({ "processes": processes }),
    ))
}

async fn shell_input(
    context: ToolContext,
    arguments: ShellInputArgs,
) -> Result<ToolResult, ToolError> {
    let provider = process_provider(&context, "shell_input")?;
    let snapshot = provider
        .input_process(
            &arguments.process_id,
            &arguments.text,
            arguments.close_stdin,
        )
        .await
        .map_err(|error| tool_execution_error("shell_input", error))?;
    process_result(&context, &snapshot).await
}

async fn shell_signal(
    context: ToolContext,
    arguments: ShellSignalArgs,
) -> Result<ToolResult, ToolError> {
    let provider = process_provider(&context, "shell_signal")?;
    let snapshot = provider
        .signal_process(&arguments.process_id, arguments.signal)
        .await
        .map_err(|error| tool_execution_error("shell_signal", error))?;
    process_result(&context, &snapshot).await
}

async fn shell_kill(
    context: ToolContext,
    arguments: ProcessIdArgs,
) -> Result<ToolResult, ToolError> {
    let provider = process_provider(&context, "shell_kill")?;
    let snapshot = provider
        .kill_process(&arguments.process_id)
        .await
        .map_err(|error| tool_execution_error("shell_kill", error))?;
    process_result(&context, &snapshot).await
}

fn shell_review_context(
    environment: ShellReviewEnvironmentContext,
    timeout_seconds: u64,
) -> ShellReviewContextSnapshot {
    ShellReviewContextSnapshot {
        timeout_seconds: Some(timeout_seconds),
        tool_call_id: None,
        tool_call_approved: false,
        default_cwd: environment.default_cwd,
        allowed_paths: environment.allowed_paths,
        shell_platform: environment.shell_platform,
        shell_executable: environment.shell_executable,
    }
}

fn process_provider(
    context: &ToolContext,
    tool: &str,
) -> Result<DynProcessShellProvider, ToolError> {
    let agent_context = context.dependency::<AgentContext>().ok_or_else(|| {
        tool_execution_error(tool, "AgentContext dependency is missing from ToolContext")
    })?;
    let handle = agent_context
        .dependencies
        .get::<ProcessShellHandle>()
        .ok_or_else(|| {
            tool_execution_error(tool, "ProcessShellHandle is missing from AgentContext")
        })?;
    Ok(handle.provider())
}

async fn process_result(
    context: &ToolContext,
    snapshot: &ShellProcessSnapshot,
) -> Result<ToolResult, ToolError> {
    let environment = maybe_environment_provider(context);
    let truncate_limit = shell_output_truncate_limit(context);
    let stdout = if let Some(provider) = environment.as_ref() {
        truncate_shell_output(
            provider.as_ref(),
            "stdout",
            &snapshot.stdout,
            truncate_limit,
        )
        .await
    } else {
        truncate_shell_output_without_file(&snapshot.stdout, truncate_limit)
    };
    let stderr = if let Some(provider) = environment.as_ref() {
        truncate_shell_output(
            provider.as_ref(),
            "stderr",
            &snapshot.stderr,
            truncate_limit,
        )
        .await
    } else {
        truncate_shell_output_without_file(&snapshot.stderr, truncate_limit)
    };
    let mut result = serde_json::json!({
        "process_id": snapshot.process_id,
        "command": snapshot.command,
        "status": snapshot.status,
        "stdout": stdout.content,
        "stderr": stderr.content,
        "return_code": snapshot.return_code,
        "metadata": snapshot.metadata,
    });
    if let Some(path) = stdout.file_path {
        result["stdout_file_path"] = serde_json::json!(path);
    }
    if let Some(path) = stderr.file_path {
        result["stderr_file_path"] = serde_json::json!(path);
    }
    Ok(ToolResult::new(
        guard_shell_result(context, environment.as_deref(), result, "shell-process").await,
    ))
}

async fn guard_shell_result(
    context: &ToolContext,
    provider: Option<&dyn EnvironmentProvider>,
    result: serde_json::Value,
    prefix: &str,
) -> serde_json::Value {
    let limit = shell_output_truncate_limit(context).max(DEFAULT_TOOL_OUTPUT_TRUNCATE_LIMIT);
    if tool_output_size(&result) <= limit {
        return result;
    }

    let full_result = dump_tool_output(&result);
    let output_path = write_tmp_output(provider, prefix, "json", full_result.as_bytes()).await;
    let guidance = output_too_large_message(
        full_result.chars().count(),
        output_path.as_deref(),
        "shell result",
    );

    let mut preview = match result {
        serde_json::Value::Object(map) => map,
        other => {
            let mut map = serde_json::Map::new();
            map.insert("result".to_string(), other);
            map
        }
    };
    preview.insert("truncated".to_string(), serde_json::Value::Bool(true));
    if let Some(path) = output_path.as_ref() {
        preview.insert(
            "output_file_path".to_string(),
            serde_json::Value::String(path.clone()),
        );
    }
    let hint = preview
        .get("hint")
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned);
    preview.insert(
        "hint".to_string(),
        serde_json::Value::String(append_guidance(hint.as_deref(), &guidance)),
    );

    let suffix = if output_path.is_some() {
        "\n...(truncated; full shell result saved in `output_file_path`)"
    } else {
        "\n...(truncated; failed to save full shell result)"
    };
    let fitted = fit_text_fields_to_limit(
        serde_json::Value::Object(preview.clone()),
        &["stdout", "stderr"],
        limit,
        suffix,
    );
    if tool_output_size(&fitted) <= limit {
        return fitted;
    }

    minimal_shell_preview(
        &serde_json::Value::Object(preview),
        output_path.as_deref(),
        &guidance,
    )
}

fn minimal_shell_preview(
    result: &serde_json::Value,
    output_path: Option<&str>,
    guidance: &str,
) -> serde_json::Value {
    let mut preview = serde_json::Map::new();
    for key in ["command", "process_id", "status", "return_code"] {
        if let Some(value) = result.get(key) {
            preview.insert(key.to_string(), value.clone());
        }
    }
    preview.insert("truncated".to_string(), serde_json::Value::Bool(true));
    preview.insert(
        "hint".to_string(),
        serde_json::Value::String(guidance.to_string()),
    );
    if let Some(path) = output_path {
        preview.insert(
            "output_file_path".to_string(),
            serde_json::Value::String(path.to_string()),
        );
    }
    serde_json::Value::Object(preview)
}

struct TruncatedOutput {
    content: String,
    file_path: Option<String>,
}

fn shell_output_truncate_limit(context: &ToolContext) -> usize {
    context.dependency::<AgentContext>().map_or_else(
        || ToolConfig::default().shell_output_truncate_limit,
        |context| context.tool_config.shell_output_truncate_limit,
    )
}

async fn truncate_shell_output(
    provider: &dyn starweaver_environment::EnvironmentProvider,
    stream_name: &str,
    content: &str,
    truncate_limit: usize,
) -> TruncatedOutput {
    if content.len() <= truncate_limit {
        return TruncatedOutput {
            content: content.to_string(),
            file_path: None,
        };
    }
    let filename = format!("{stream_name}-{}.log", Uuid::new_v4().simple());
    provider
        .write_tmp_file(&filename, content.as_bytes())
        .await
        .map_or_else(
            |_| truncate_shell_output_without_file(content, truncate_limit),
            |path| TruncatedOutput {
                content: format!(
                    "{}\n...(truncated, full output at `{stream_name}_file_path`)",
                    content.chars().take(truncate_limit).collect::<String>()
                ),
                file_path: Some(path),
            },
        )
}

fn truncate_shell_output_without_file(content: &str, truncate_limit: usize) -> TruncatedOutput {
    if content.len() <= truncate_limit {
        return TruncatedOutput {
            content: content.to_string(),
            file_path: None,
        };
    }
    TruncatedOutput {
        content: format!(
            "{}\n...(truncated)",
            content.chars().take(truncate_limit).collect::<String>()
        ),
        file_path: None,
    }
}
