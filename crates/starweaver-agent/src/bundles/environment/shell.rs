use std::sync::Arc;

use starweaver_context::AgentContext;
use starweaver_environment::{DynProcessShellProvider, ShellCommand, ShellProcessSnapshot};
use starweaver_tools::{
    DynToolset, EmptyToolArgs, StaticToolset, ToolContext, ToolError, ToolInstruction, ToolResult,
};
use uuid::Uuid;

use super::{
    args::{ProcessIdArgs, ShellExecArgs, ShellInputArgs, ShellSignalArgs, ShellWaitArgs},
    handle::{environment_provider, maybe_environment_provider},
};
use crate::bundles::helpers::{
    static_tool, static_tool_with_metadata, tool_execution_error, tool_metadata,
};

const OUTPUT_TRUNCATE_LIMIT: usize = 20_000;

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
pub fn shell_tools() -> DynToolset {
    let approval_metadata = tool_metadata("shell", false, true);

    Arc::new(
        StaticToolset::new("shell")
            .with_id("shell")
            .with_instruction(ToolInstruction::new(
                "shell",
                "Shell tools execute through the active AgentContext environment policy. Use shell_exec for bounded one-shot commands, set background=true for long-running work, and use shell_wait, shell_status, shell_input, shell_signal, or shell_kill for durable background handles when the provider supports them. Large stdout and stderr are saved via the active environment provider tmp-file abstraction.",
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
    let environment = arguments.environment.clone().unwrap_or_default();
    let shell_command = ShellCommand {
        command: arguments.command.clone(),
        timeout_seconds: Some(arguments.timeout_seconds),
        cwd: arguments.cwd.clone(),
        environment: environment.clone(),
    };
    if arguments.background {
        let provider = process_provider(&context, "shell_exec")?;
        let snapshot = provider
            .start_process(shell_command)
            .await
            .map_err(|error| tool_execution_error("shell_exec", error))?;
        return process_result(&context, &snapshot).await;
    }
    let provider = environment_provider(&context, "shell_exec")?;
    let output = provider
        .run_shell(shell_command)
        .await
        .map_err(|error| tool_execution_error("shell_exec", error))?;
    let stdout = truncate_shell_output(provider.as_ref(), "stdout", &output.stdout).await;
    let stderr = truncate_shell_output(provider.as_ref(), "stderr", &output.stderr).await;
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
    Ok(ToolResult::new(result))
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
    let stdout = if let Some(provider) = environment.as_ref() {
        truncate_shell_output(provider.as_ref(), "stdout", &snapshot.stdout).await
    } else {
        truncate_shell_output_without_file(&snapshot.stdout)
    };
    let stderr = if let Some(provider) = environment.as_ref() {
        truncate_shell_output(provider.as_ref(), "stderr", &snapshot.stderr).await
    } else {
        truncate_shell_output_without_file(&snapshot.stderr)
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
    Ok(ToolResult::new(result))
}

struct TruncatedOutput {
    content: String,
    file_path: Option<String>,
}

async fn truncate_shell_output(
    provider: &dyn starweaver_environment::EnvironmentProvider,
    stream_name: &str,
    content: &str,
) -> TruncatedOutput {
    if content.len() <= OUTPUT_TRUNCATE_LIMIT {
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
            |_| truncate_shell_output_without_file(content),
            |path| TruncatedOutput {
                content: format!(
                    "{}\n...(truncated, full output at `{stream_name}_file_path`)",
                    content
                        .chars()
                        .take(OUTPUT_TRUNCATE_LIMIT)
                        .collect::<String>()
                ),
                file_path: Some(path),
            },
        )
}

fn truncate_shell_output_without_file(content: &str) -> TruncatedOutput {
    if content.len() <= OUTPUT_TRUNCATE_LIMIT {
        return TruncatedOutput {
            content: content.to_string(),
            file_path: None,
        };
    }
    TruncatedOutput {
        content: format!(
            "{}\n...(truncated)",
            content
                .chars()
                .take(OUTPUT_TRUNCATE_LIMIT)
                .collect::<String>()
        ),
        file_path: None,
    }
}
