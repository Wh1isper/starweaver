use std::sync::Arc;

use starweaver_tools::{
    DynToolset, EmptyToolArgs, StaticToolset, ToolContext, ToolError, ToolInstruction, ToolResult,
};

use super::{
    args::{ProcessIdArgs, ShellExecArgs, ShellInputArgs, ShellSignalArgs, ShellWaitArgs},
    common::operation,
    handle::environment_provider,
};
use crate::bundles::helpers::{
    static_tool, static_tool_with_metadata, tool_execution_error, tool_metadata,
};

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
