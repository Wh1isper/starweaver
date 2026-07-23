use std::{collections::BTreeMap, future::Future, sync::Arc, thread, time::Duration};

use starweaver_context::{
    AgentContext, CONTEXT_USAGE_CAPABILITY, HostCapabilities, ShellEnvironmentSnapshot, ToolConfig,
    ToolRuntimeSnapshot,
};
use starweaver_environment::{
    DynProcessShellProvider, EnvironmentProvider, EnvironmentResult, ShellCommand, ShellOutput,
    ShellProcessSnapshot, ShellProcessStatus, ShellReviewEnvironmentContext,
};
use starweaver_tools::{
    DynToolset, EmptyToolArgs, StaticToolset, ToolContext, ToolDependencyRequirements, ToolError,
    ToolInstruction, ToolResult,
};
use uuid::Uuid;

use super::{
    args::{ProcessIdArgs, ShellExecArgs, ShellInputArgs, ShellSignalArgs, ShellWaitArgs},
    handle::{environment_provider, maybe_environment_provider},
    shell_review::{ShellReviewContextSnapshot, review_shell_command_or_block},
};
use crate::bundles::helpers::{
    static_sequential_tool_with_metadata, static_tool_with_metadata, tool_environment_error,
    tool_execution_error, tool_invalid_arguments, tool_metadata_with_dependencies, tool_user_error,
};
use crate::bundles::output::{
    DEFAULT_TOOL_OUTPUT_TRUNCATE_LIMIT, append_guidance, dump_tool_output,
    fit_text_fields_to_limit, output_too_large_message, tool_output_size, write_scratch_output,
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
    let shell_requirements = ToolDependencyRequirements::filtered(Vec::<String>::new(), true)
        .with_context_capabilities([CONTEXT_USAGE_CAPABILITY]);
    let approval_metadata =
        tool_metadata_with_dependencies("shell", false, true, &shell_requirements);
    let shell_metadata =
        tool_metadata_with_dependencies("shell", false, false, &shell_requirements);

    Arc::new(
        StaticToolset::new("shell")
            .with_id("shell")
            .with_instruction(ToolInstruction::new(
                "shell",
                r#"<shell-guidelines>
Check the runtime `<environment-context><shell-execution>` context for the active shell dialect before relying on shell-specific syntax.

Large outputs may be saved to stdout_file_path, stderr_file_path, or output_file_path. When a shell command needs to create scratch files itself, write them under `$TMPDIR` rather than hard-coded `/tmp/...`; local providers map `$TMPDIR` to a provider-managed, session-scoped directory that file tools can read.

<background-mode>
Set background=true for long-running commands such as builds, servers, and test suites. Completed background processes are automatically reported in context; use shell_wait only when you need results before proceeding, and use timeout_seconds=0 to poll without blocking.
</background-mode>

Avoid:
- find/grep for searching when glob or grep tools can do the job.
- cat/head/tail/ls to inspect files when view and ls can do the job; bounded shell readers are acceptable when view cannot handle the file size or format.
- cd command; use the cwd parameter instead.
</shell-guidelines>"#,
            ))
            .with_tools([
                static_sequential_tool_with_metadata(
                    "shell_exec",
                    "Run a provider-scoped shell command. Set background=true for a durable background handle.",
                    approval_metadata,
                    shell_exec,
                ),
                static_sequential_tool_with_metadata(
                    "shell_wait",
                    "Wait for or poll a background shell process.",
                    shell_metadata.clone(),
                    shell_wait,
                ),
                static_tool_with_metadata(
                    "shell_status",
                    "List background shell process status.",
                    shell_metadata.clone(),
                    shell_status,
                ),
                static_sequential_tool_with_metadata(
                    "shell_input",
                    "Write text to a background process stdin.",
                    shell_metadata.clone(),
                    shell_input,
                ),
                static_sequential_tool_with_metadata(
                    "shell_signal",
                    "Send a Unix signal to a background process.",
                    shell_metadata.clone(),
                    shell_signal,
                ),
                static_sequential_tool_with_metadata(
                    "shell_kill",
                    "Terminate and clean up a background shell process.",
                    shell_metadata,
                    shell_kill,
                ),
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
        let (snapshot, mut process_guard) =
            start_process_with_cleanup_handoff(provider, shell_command).await?;
        let result = process_result(&context, &snapshot).await?;
        process_guard.disarm();
        return Ok(result);
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
    if foreground_process_shell_supported(provider.as_ref())
        && let Some(process_provider) = maybe_process_provider(&context)
    {
        return shell_exec_foreground_process(
            &context,
            provider.as_ref(),
            process_provider,
            &arguments,
            shell_command,
            &environment,
        )
        .await;
    }
    let output = provider
        .run_shell(shell_command)
        .await
        .map_err(|error| tool_environment_error("shell_exec", error))?;
    shell_exec_output_result(
        &context,
        provider.as_ref(),
        &arguments,
        &environment,
        output,
    )
    .await
}

fn foreground_process_shell_supported(provider: &dyn EnvironmentProvider) -> bool {
    provider.shell_review_context().shell_platform.as_deref() != Some("virtual")
}

async fn shell_exec_foreground_process(
    context: &ToolContext,
    environment_provider: &dyn EnvironmentProvider,
    process_provider: DynProcessShellProvider,
    arguments: &ShellExecArgs,
    shell_command: ShellCommand,
    environment: &BTreeMap<String, String>,
) -> Result<ToolResult, ToolError> {
    let (started, mut process_guard) =
        start_process_with_cleanup_handoff(process_provider.clone(), shell_command).await?;
    let snapshot = wait_process_polling(
        context,
        process_provider.clone(),
        "shell_exec",
        &started.process_id,
        arguments.timeout_seconds,
        true,
    )
    .await?;
    process_guard.disarm();
    let output = shell_output_from_snapshot(&snapshot);
    shell_exec_output_result(
        context,
        environment_provider,
        arguments,
        environment,
        output,
    )
    .await
}

async fn shell_exec_output_result(
    context: &ToolContext,
    provider: &dyn EnvironmentProvider,
    arguments: &ShellExecArgs,
    environment: &BTreeMap<String, String>,
    output: ShellOutput,
) -> Result<ToolResult, ToolError> {
    let truncate_limit = shell_output_truncate_limit(context);
    let stdout = truncate_shell_output(provider, "stdout", &output.stdout, truncate_limit).await;
    let stderr = truncate_shell_output(provider, "stderr", &output.stderr, truncate_limit).await;
    let mut result = serde_json::json!({
        "command": &arguments.command,
        "timeout_seconds": arguments.timeout_seconds,
        "environment": environment,
        "cwd": &arguments.cwd,
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
    if !output.metadata.is_empty() {
        result["metadata"] = serde_json::json!(output.metadata);
    }
    Ok(ToolResult::new(
        guard_shell_result(context, Some(provider), result, "shell-exec").await,
    ))
}

// A provider can register the process before its start future hands the snapshot
// back to this tool. Keep that handoff in a detached task whose output already
// owns cleanup, so caller cancellation still attempts to clean up an unobserved process.
async fn start_process_with_cleanup_handoff(
    provider: DynProcessShellProvider,
    command: ShellCommand,
) -> Result<(ShellProcessSnapshot, ProcessCleanupGuard), ToolError> {
    let start_provider = provider.clone();
    await_process_start_with_cleanup(provider, async move {
        start_provider.start_process(command).await
    })
    .await
}

async fn await_process_start_with_cleanup<F>(
    provider: DynProcessShellProvider,
    start: F,
) -> Result<(ShellProcessSnapshot, ProcessCleanupGuard), ToolError>
where
    F: Future<Output = EnvironmentResult<ShellProcessSnapshot>> + Send + 'static,
{
    tokio::spawn(async move {
        start.await.map(|snapshot| {
            let process_id = snapshot.process_id.clone();
            (snapshot, ProcessCleanupGuard::new(provider, process_id))
        })
    })
    .await
    .map_err(|error| tool_execution_error("shell_exec", error))?
    .map_err(|error| tool_environment_error("shell_exec", error))
}

struct ProcessCleanupGuard {
    provider: Option<DynProcessShellProvider>,
    process_id: String,
}

impl ProcessCleanupGuard {
    fn new(provider: DynProcessShellProvider, process_id: String) -> Self {
        Self {
            provider: Some(provider),
            process_id,
        }
    }

    fn disarm(&mut self) {
        self.provider = None;
    }
}

impl Drop for ProcessCleanupGuard {
    fn drop(&mut self) {
        let Some(provider) = self.provider.take() else {
            return;
        };
        let process_id = self.process_id.clone();
        std::mem::drop(thread::spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            let _ = runtime.block_on(provider.kill_process(&process_id));
        }));
    }
}

async fn wait_process_polling(
    context: &ToolContext,
    provider: DynProcessShellProvider,
    tool: &str,
    process_id: &str,
    timeout_seconds: u64,
    kill_on_timeout: bool,
) -> Result<ShellProcessSnapshot, ToolError> {
    let cancellation_token = context.cancellation_token();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_seconds);
    loop {
        if cancellation_token.is_cancelled() {
            let _ = provider.kill_process(process_id).await;
            return Err(ToolError::Cancelled {
                tool: tool.to_string(),
                reason: "agent run cancellation requested".to_string(),
            });
        }
        let snapshot = provider
            .wait_process(process_id, 0)
            .await
            .map_err(|error| tool_environment_error(tool, error))?;
        if snapshot.status != ShellProcessStatus::Running {
            return Ok(snapshot);
        }
        if timeout_seconds == 0 {
            if kill_on_timeout {
                let mut snapshot = provider
                    .kill_process(process_id)
                    .await
                    .map_err(|error| tool_environment_error(tool, error))?;
                mark_process_timed_out(&mut snapshot, timeout_seconds);
                return Ok(snapshot);
            }
            return Ok(snapshot);
        }
        if tokio::time::Instant::now() >= deadline {
            if kill_on_timeout {
                let mut snapshot = provider
                    .kill_process(process_id)
                    .await
                    .map_err(|error| tool_environment_error(tool, error))?;
                mark_process_timed_out(&mut snapshot, timeout_seconds);
                return Ok(snapshot);
            }
            return Ok(snapshot);
        }
        tokio::select! {
            () = cancellation_token.cancelled() => {
                let _ = provider.kill_process(process_id).await;
                return Err(ToolError::Cancelled {
                    tool: tool.to_string(),
                    reason: "agent run cancellation requested".to_string(),
                });
            }
            () = tokio::time::sleep(Duration::from_millis(25)) => {}
        }
    }
}

fn shell_output_from_snapshot(snapshot: &ShellProcessSnapshot) -> ShellOutput {
    let mut metadata = snapshot.metadata.clone();
    metadata.insert(
        "process_id".to_string(),
        serde_json::json!(&snapshot.process_id),
    );
    metadata.insert(
        "process_status".to_string(),
        serde_json::json!(&snapshot.status),
    );
    ShellOutput {
        status: snapshot.return_code.unwrap_or(match &snapshot.status {
            ShellProcessStatus::Completed => 0,
            ShellProcessStatus::Running
            | ShellProcessStatus::Failed
            | ShellProcessStatus::Killed => -1,
        }),
        stdout: snapshot.stdout.clone(),
        stderr: snapshot.stderr.clone(),
        metadata,
    }
}

fn mark_process_timed_out(snapshot: &mut ShellProcessSnapshot, timeout_seconds: u64) {
    snapshot
        .metadata
        .insert("timed_out".to_string(), serde_json::json!(true));
    snapshot.metadata.insert(
        "timeout_seconds".to_string(),
        serde_json::json!(timeout_seconds),
    );
    if !snapshot.stderr.is_empty() && !snapshot.stderr.ends_with('\n') {
        snapshot.stderr.push('\n');
    }
    snapshot.stderr.push_str("shell command timed out");
}

fn merged_shell_environment(
    context: &ToolContext,
    per_call: Option<BTreeMap<String, String>>,
) -> BTreeMap<String, String> {
    let mut environment = context
        .dependency::<ShellEnvironmentSnapshot>()
        .map_or_else(
            || {
                context
                    .dependency::<ToolRuntimeSnapshot>()
                    .map_or_else(BTreeMap::new, |runtime| runtime.shell_environment().clone())
            },
            |snapshot| snapshot.environment().clone(),
        );
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
    let snapshot = wait_process_polling(
        &context,
        provider,
        "shell_wait",
        &arguments.process_id,
        arguments.timeout_seconds,
        false,
    )
    .await?;
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
        .map_err(|error| tool_environment_error("shell_status", error))?;
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
        .map_err(|error| tool_environment_error("shell_input", error))?;
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
        .map_err(|error| tool_environment_error("shell_signal", error))?;
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
        .map_err(|error| tool_environment_error("shell_kill", error))?;
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
    maybe_process_provider(context).ok_or_else(|| {
        tool_user_error(
            tool,
            "ProcessShellHandle dependency is missing from ToolContext",
        )
    })
}

fn maybe_process_provider(context: &ToolContext) -> Option<DynProcessShellProvider> {
    context
        .dependency::<HostCapabilities>()
        .and_then(|capabilities| capabilities.get::<ProcessShellHandle>())
        .or_else(|| context.dependency::<ProcessShellHandle>())
        .map(|handle| handle.provider())
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
    let output_path = write_scratch_output(provider, prefix, "json", full_result.as_bytes()).await;
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
    context.dependency::<ToolRuntimeSnapshot>().map_or_else(
        || ToolConfig::default().shell_output_truncate_limit,
        |runtime| runtime.tool_config().shell_output_truncate_limit,
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
        .write_scratch_file(&filename, content.as_bytes())
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

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use starweaver_context::DependencyStore;
    use starweaver_core::{ConversationId, RunId};
    use starweaver_environment::{
        EnvironmentPolicy, FilePolicy, LocalEnvironmentProvider, ShellPolicy,
    };
    use tokio::sync::oneshot;

    use super::*;

    #[tokio::test]
    async fn process_start_handoff_cleans_up_when_caller_is_cancelled() {
        let root = tempfile::tempdir().unwrap();
        let local_provider = Arc::new(
            LocalEnvironmentProvider::new(root.path())
                .unwrap()
                .with_policy(EnvironmentPolicy {
                    files: FilePolicy::read_only(),
                    shell: ShellPolicy::allow_all(),
                }),
        );
        let provider: DynProcessShellProvider = local_provider;
        let start_provider = provider.clone();
        let (started_sender, started_receiver) = oneshot::channel();
        let (release_sender, release_receiver) = oneshot::channel();
        #[cfg(windows)]
        let command = "ping -n 31 127.0.0.1 >NUL";
        #[cfg(not(windows))]
        let command = "exec sleep 30";
        let start = async move {
            let snapshot = start_provider
                .start_process(ShellCommand::shell(command))
                .await?;
            let _ = started_sender.send(snapshot.process_id.clone());
            let _ = release_receiver.await;
            Ok(snapshot)
        };
        let caller = tokio::spawn(await_process_start_with_cleanup(provider.clone(), start));
        let process_id = tokio::time::timeout(Duration::from_secs(10), started_receiver)
            .await
            .expect("process start should not time out")
            .expect("process start should be observed");

        caller.abort();
        match caller.await {
            Err(error) => assert!(error.is_cancelled()),
            Ok(_) => panic!("process start caller should be cancelled"),
        }
        release_sender
            .send(())
            .expect("detached process start should remain alive");

        let status = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let snapshot = provider.wait_process(&process_id, 0).await.unwrap();
                if snapshot.status != ShellProcessStatus::Running {
                    break snapshot.status;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("orphaned process cleanup should not time out");
        assert_eq!(status, ShellProcessStatus::Killed);
    }

    #[test]
    fn shell_environment_prefers_dedicated_projection_and_keeps_legacy_fallback() {
        let mut agent_context = AgentContext::default();
        agent_context
            .tools
            .shell_environment
            .insert("SOURCE".to_string(), "legacy".to_string());
        let legacy_runtime = agent_context.tool_runtime_snapshot();

        let mut legacy_dependencies = DependencyStore::new();
        legacy_dependencies.insert(legacy_runtime.clone());
        let legacy_context = ToolContext::new(RunId::default(), ConversationId::default(), 0)
            .with_dependencies(legacy_dependencies);
        assert_eq!(
            merged_shell_environment(&legacy_context, None)["SOURCE"],
            "legacy"
        );

        agent_context
            .tools
            .shell_environment
            .insert("SOURCE".to_string(), "dedicated".to_string());
        let mut filtered_dependencies = DependencyStore::new();
        filtered_dependencies.insert(legacy_runtime);
        filtered_dependencies.insert(agent_context.shell_environment_snapshot());
        let filtered_context = ToolContext::new(RunId::default(), ConversationId::default(), 0)
            .with_dependencies(filtered_dependencies);
        let environment = merged_shell_environment(
            &filtered_context,
            Some(BTreeMap::from([(
                "PER_CALL".to_string(),
                "override".to_string(),
            )])),
        );

        assert_eq!(environment["SOURCE"], "dedicated");
        assert_eq!(environment["PER_CALL"], "override");
    }
}
