use std::fmt::Write as _;

use clap_complete::Shell;
use serde_json::{Value, json};
use starweaver_session::{
    ApprovalRecord, ApprovalStatus, DeferredToolRecord, SessionSearchCoverageState,
    SessionSearchPage,
};
use starweaver_stream::{DisplayMessage, DisplayMessageKind};

use super::{PromptRunExecution, render_json_lines};
use crate::{
    CliError, CliResult,
    args::OutputMode,
    display_preview::run_output_preview,
    local_store::{RunSummary, SessionSummary, TrimReport},
};

pub(super) fn render_sessions(
    sessions: &[SessionSummary],
    output: OutputMode,
) -> CliResult<String> {
    match output {
        OutputMode::Text => {
            let mut lines = String::new();
            for session in sessions {
                let _ = writeln!(
                    lines,
                    "{} profile={} runs={} status={}",
                    session.session_id,
                    session.profile.as_deref().unwrap_or_default(),
                    session.run_count,
                    session.status
                );
            }
            Ok(lines)
        }
        OutputMode::DisplayJsonl | OutputMode::AguiJsonl => sessions
            .iter()
            .map(|session| serde_json::to_string(session).map(|line| format!("{line}\n")))
            .collect::<Result<String, _>>()
            .map_err(CliError::from),
        OutputMode::Json => Ok(format!(
            "{}\n",
            serde_json::to_string(&json!({"sessions": sessions, "status": "list"}))?
        )),
        OutputMode::Silent => Ok(format!("sessions={}\nstatus=list\n", sessions.len())),
    }
}

pub(super) fn render_session_search(
    page: &SessionSearchPage,
    output: OutputMode,
) -> CliResult<String> {
    match output {
        OutputMode::Text => {
            let mut lines = String::new();
            for hit in &page.hits {
                let source = serde_json::to_value(hit.source)?
                    .as_str()
                    .unwrap_or("unknown")
                    .to_string();
                let _ = writeln!(
                    lines,
                    "session_id={} run_id={} updated={} source={} title={} snippet={}",
                    hit.session.session_id.as_str(),
                    hit.run_id
                        .as_ref()
                        .map_or("", starweaver_core::RunId::as_str),
                    hit.session.updated_at.to_rfc3339(),
                    source,
                    hit.session.title.as_deref().unwrap_or_default(),
                    hit.snippet
                        .as_ref()
                        .map_or("", |snippet| snippet.text.as_str())
                );
            }
            if page.coverage.state != SessionSearchCoverageState::Complete {
                let _ = writeln!(
                    lines,
                    "warning: session search coverage is {:?}",
                    page.coverage.state
                );
                for warning in &page.coverage.warnings {
                    let _ = writeln!(lines, "warning: {}", warning.message);
                }
            }
            if let Some(cursor) = page.next_cursor.as_deref() {
                let _ = writeln!(lines, "next_cursor={cursor}");
            }
            Ok(lines)
        }
        OutputMode::DisplayJsonl | OutputMode::AguiJsonl => {
            let mut lines = String::new();
            for hit in &page.hits {
                lines.push_str(&serde_json::to_string(hit)?);
                lines.push('\n');
            }
            lines.push_str(&serde_json::to_string(&json!({
                "type": "session_search_page",
                "nextCursor": page.next_cursor,
                "coverage": page.coverage,
            }))?);
            lines.push('\n');
            Ok(lines)
        }
        OutputMode::Json => Ok(format!("{}\n", serde_json::to_string(page)?)),
        OutputMode::Silent => Ok(format!(
            "hits={}\nnext_cursor={}\ncoverage={:?}\nstatus=search\n",
            page.hits.len(),
            page.next_cursor.as_deref().unwrap_or_default(),
            page.coverage.state
        )),
    }
}

pub(super) fn render_session_show(
    session: &Value,
    runs: &[RunSummary],
    output: OutputMode,
) -> CliResult<String> {
    match output {
        OutputMode::Text => {
            let mut lines = format!(
                "session_id={} profile={} status={}\n",
                session["session_id"].as_str().unwrap_or_default(),
                session["profile"].as_str().unwrap_or_default(),
                session["status"].as_str().unwrap_or_default()
            );
            for run in runs {
                let _ = writeln!(
                    lines,
                    "run_id={} sequence={} status={} preview={}",
                    run.run_id,
                    run.sequence_no,
                    run.status,
                    run.output_preview.as_deref().unwrap_or_default()
                );
            }
            Ok(lines)
        }
        OutputMode::DisplayJsonl | OutputMode::AguiJsonl => {
            let mut lines = String::new();
            lines.push_str(&serde_json::to_string(session)?);
            lines.push('\n');
            for run in runs {
                lines.push_str(&serde_json::to_string(run)?);
                lines.push('\n');
            }
            Ok(lines)
        }
        OutputMode::Json => Ok(format!(
            "{}\n",
            serde_json::to_string(&json!({"session": session, "runs": runs}))?
        )),
        OutputMode::Silent => Ok(format!(
            "session_id={}\nruns={}\nstatus=shown\n",
            session["session_id"].as_str().unwrap_or_default(),
            runs.len()
        )),
    }
}

pub(super) fn render_display_jsonl(messages: &[DisplayMessage]) -> CliResult<String> {
    messages
        .iter()
        .map(DisplayMessage::to_jsonl_line)
        .collect::<Result<String, _>>()
        .map_err(CliError::from)
}

pub(super) fn render_agui_jsonl(messages: &[DisplayMessage]) -> CliResult<String> {
    messages
        .iter()
        .filter_map(display_message_to_agui_event)
        .map(|event| serde_json::to_string(&event).map(|line| format!("{line}\n")))
        .collect::<Result<String, _>>()
        .map_err(CliError::from)
}

pub(super) fn render_prompt_run_json(execution: &PromptRunExecution) -> CliResult<String> {
    let output_preview = run_output_preview(&execution.messages).unwrap_or_default();
    let latest_sequence = execution.messages.last().map(|message| message.sequence);
    Ok(format!(
        "{}\n",
        serde_json::to_string(&json!({
            "sessionId": execution.session_id,
            "runId": execution.run_id,
            "status": execution.status,
            "outputPreview": output_preview,
            "latestCursor": latest_sequence.map(|sequence| json!({
                "scope": format!("run:{}", execution.run_id),
                "sequence": sequence,
            })),
        }))?
    ))
}

#[allow(clippy::too_many_lines)]
pub(super) fn display_message_to_agui_event(message: &DisplayMessage) -> Option<Value> {
    let mut event = match message.kind {
        DisplayMessageKind::RunQueued => {
            let value = json!({"sequence_no": message.payload.get("sequence_no").cloned()});
            custom_agui_event("starweaver.run_queued", message, &value)
        }
        DisplayMessageKind::RunStarted => json!({
            "type": "RUN_STARTED",
            "threadId": message.session_id.as_str(),
            "runId": message.run_id.as_str(),
        }),
        DisplayMessageKind::AssistantTextStart => {
            if is_reasoning_message(message) {
                json!({
                    "type": "REASONING_MESSAGE_START",
                    "messageId": message_id(message),
                    "role": "reasoning",
                })
            } else {
                json!({
                    "type": "TEXT_MESSAGE_START",
                    "messageId": message_id(message),
                    "role": message.payload.get("role").and_then(Value::as_str).unwrap_or("assistant"),
                    "name": message.agent_name,
                })
            }
        }
        DisplayMessageKind::AssistantTextDelta => {
            if is_reasoning_message(message) {
                json!({
                    "type": "REASONING_MESSAGE_CHUNK",
                    "messageId": message_id(message),
                    "delta": message_delta(message),
                })
            } else {
                json!({
                    "type": "TEXT_MESSAGE_CHUNK",
                    "messageId": message_id(message),
                    "role": "assistant",
                    "name": message.agent_name,
                    "delta": message_delta(message),
                })
            }
        }
        DisplayMessageKind::AssistantTextEnd => {
            if is_reasoning_message(message) {
                json!({
                    "type": "REASONING_MESSAGE_END",
                    "messageId": message_id(message),
                })
            } else {
                json!({
                    "type": "TEXT_MESSAGE_END",
                    "messageId": message_id(message),
                })
            }
        }
        DisplayMessageKind::ToolCallStart => json!({
            "type": "TOOL_CALL_START",
            "toolCallId": tool_call_id(message),
            "toolCallName": tool_call_name(message),
            "parentMessageId": message.payload.get("parent_message_id").cloned(),
        }),
        DisplayMessageKind::ToolCallDelta => json!({
            "type": "TOOL_CALL_CHUNK",
            "toolCallId": tool_call_id(message),
            "toolCallName": tool_call_name(message),
            "delta": message_delta(message),
        }),
        DisplayMessageKind::ToolCallEnd => json!({
            "type": "TOOL_CALL_END",
            "toolCallId": tool_call_id(message),
        }),
        DisplayMessageKind::ToolResult => json!({
            "type": "TOOL_CALL_RESULT",
            "messageId": format!("{}:result", tool_call_id(message)),
            "toolCallId": tool_call_id(message),
            "toolCallName": tool_call_name(message),
            "content": message.payload.get("content").cloned().unwrap_or_else(|| json!(message.preview)),
            "role": "tool",
            "error": message.payload.get("is_error").and_then(Value::as_bool).unwrap_or(false),
        }),
        DisplayMessageKind::RunCompleted => {
            let output = message
                .payload
                .get("output")
                .cloned()
                .or_else(|| message.preview.clone().map(Value::String));
            json!({
                "type": "RUN_FINISHED",
                "threadId": message.session_id.as_str(),
                "runId": message.run_id.as_str(),
                "result": output.map(|output| json!({"output_text": output})),
            })
        }
        DisplayMessageKind::RunFailed => json!({
            "type": "RUN_ERROR",
            "message": message.preview.as_deref().unwrap_or("run failed"),
            "code": message.payload.get("code").and_then(Value::as_str),
        }),
        DisplayMessageKind::RunCancelled => {
            custom_agui_event("starweaver.run_cancelled", message, &message.payload)
        }
        DisplayMessageKind::ApprovalRequested
        | DisplayMessageKind::ApprovalResolved
        | DisplayMessageKind::HitlResolved
        | DisplayMessageKind::HitlDiagnostic
        | DisplayMessageKind::Checkpoint
        | DisplayMessageKind::ToolsUnavailable
        | DisplayMessageKind::ToolSearchLoaded
        | DisplayMessageKind::ToolSearchInitialized
        | DisplayMessageKind::ToolSearchRefreshed
        | DisplayMessageKind::ToolSearchInvalidated
        | DisplayMessageKind::ToolSearchFailed
        | DisplayMessageKind::ToolSearchNoMatch
        | DisplayMessageKind::ToolsetInitialized
        | DisplayMessageKind::ToolsetUnavailable
        | DisplayMessageKind::ToolsetFailed
        | DisplayMessageKind::ToolsetRefreshed
        | DisplayMessageKind::ToolsetClosed
        | DisplayMessageKind::SkillsScanned
        | DisplayMessageKind::SkillActivated
        | DisplayMessageKind::SkillsReloaded
        | DisplayMessageKind::SubagentStarted
        | DisplayMessageKind::SubagentCompleted
        | DisplayMessageKind::SubagentFailed
        | DisplayMessageKind::CompactionStarted
        | DisplayMessageKind::CompactionCompleted
        | DisplayMessageKind::CompactionFailed
        | DisplayMessageKind::HandoffStarted
        | DisplayMessageKind::HandoffCompleted
        | DisplayMessageKind::HandoffFailed
        | DisplayMessageKind::SteeringSubmitted
        | DisplayMessageKind::SteeringReceived
        | DisplayMessageKind::GoalIteration
        | DisplayMessageKind::GoalCompleted
        | DisplayMessageKind::TaskSnapshot
        | DisplayMessageKind::TaskEvent
        | DisplayMessageKind::NoteEvent
        | DisplayMessageKind::FileEvent
        | DisplayMessageKind::MediaEvent
        | DisplayMessageKind::HostEvent => custom_agui_event(
            display_extension_name(message.kind),
            message,
            &message.payload,
        ),
    };
    strip_null_object_fields(&mut event);
    event.as_object_mut().map(|object| {
        object.insert(
            "timestamp".to_string(),
            json!(message.timestamp.timestamp_millis()),
        );
        object.insert("starweaverSequence".to_string(), json!(message.sequence));
    })?;
    Some(event)
}

fn custom_agui_event(name: &str, message: &DisplayMessage, value: &Value) -> Value {
    json!({
        "type": "CUSTOM",
        "name": name,
        "value": {
            "run_id": message.run_id.as_str(),
            "session_id": message.session_id.as_str(),
            "payload": value,
            "preview": message.preview,
        }
    })
}

const fn display_extension_name(kind: DisplayMessageKind) -> &'static str {
    match kind {
        DisplayMessageKind::ApprovalRequested => "starweaver.approval_requested",
        DisplayMessageKind::ApprovalResolved => "starweaver.approval_resolved",
        DisplayMessageKind::HitlResolved => "starweaver.hitl_resolved",
        DisplayMessageKind::HitlDiagnostic => "starweaver.hitl_diagnostic",
        DisplayMessageKind::Checkpoint => "starweaver.checkpoint",
        DisplayMessageKind::ToolsUnavailable => "starweaver.tools_unavailable",
        DisplayMessageKind::ToolSearchLoaded => "starweaver.tool_search_loaded",
        DisplayMessageKind::ToolSearchInitialized => "starweaver.tool_search_initialized",
        DisplayMessageKind::ToolSearchRefreshed => "starweaver.tool_search_refreshed",
        DisplayMessageKind::ToolSearchInvalidated => "starweaver.tool_search_invalidated",
        DisplayMessageKind::ToolSearchFailed => "starweaver.tool_search_failed",
        DisplayMessageKind::ToolSearchNoMatch => "starweaver.tool_search_no_match",
        DisplayMessageKind::ToolsetInitialized => "starweaver.toolset_initialized",
        DisplayMessageKind::ToolsetUnavailable => "starweaver.toolset_unavailable",
        DisplayMessageKind::ToolsetFailed => "starweaver.toolset_failed",
        DisplayMessageKind::ToolsetRefreshed => "starweaver.toolset_refreshed",
        DisplayMessageKind::ToolsetClosed => "starweaver.toolset_closed",
        DisplayMessageKind::SkillsScanned => "starweaver.skills_scanned",
        DisplayMessageKind::SkillActivated => "starweaver.skill_activated",
        DisplayMessageKind::SkillsReloaded => "starweaver.skills_reloaded",
        DisplayMessageKind::SubagentStarted => "starweaver.subagent_started",
        DisplayMessageKind::SubagentCompleted => "starweaver.subagent_completed",
        DisplayMessageKind::SubagentFailed => "starweaver.subagent_failed",
        DisplayMessageKind::CompactionStarted => "starweaver.compaction_started",
        DisplayMessageKind::CompactionCompleted => "starweaver.compaction_completed",
        DisplayMessageKind::CompactionFailed => "starweaver.compaction_failed",
        DisplayMessageKind::HandoffStarted => "starweaver.handoff_started",
        DisplayMessageKind::HandoffCompleted => "starweaver.handoff_completed",
        DisplayMessageKind::HandoffFailed => "starweaver.handoff_failed",
        DisplayMessageKind::SteeringSubmitted => "starweaver.steering_submitted",
        DisplayMessageKind::SteeringReceived => "starweaver.steering_received",
        DisplayMessageKind::GoalIteration => "starweaver.goal_iteration",
        DisplayMessageKind::GoalCompleted => "starweaver.goal_completed",
        DisplayMessageKind::TaskSnapshot => "starweaver.task_snapshot",
        DisplayMessageKind::TaskEvent => "starweaver.task_event",
        DisplayMessageKind::NoteEvent => "starweaver.note_event",
        DisplayMessageKind::FileEvent => "starweaver.file_event",
        DisplayMessageKind::MediaEvent => "starweaver.media_event",
        DisplayMessageKind::HostEvent => "starweaver.host_event",
        _ => "starweaver.display_message",
    }
}

fn message_id(message: &DisplayMessage) -> String {
    message
        .payload
        .get("message_id")
        .and_then(Value::as_str)
        .map_or_else(
            || format!("{}:message:{}", message.run_id.as_str(), message.sequence),
            ToString::to_string,
        )
}

fn tool_call_id(message: &DisplayMessage) -> String {
    message
        .payload
        .get("tool_call_id")
        .and_then(Value::as_str)
        .map_or_else(
            || format!("{}:tool:{}", message.run_id.as_str(), message.sequence),
            ToString::to_string,
        )
}

fn tool_call_name(message: &DisplayMessage) -> Option<String> {
    message
        .payload
        .get("tool_name")
        .or_else(|| message.payload.get("name"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn message_delta(message: &DisplayMessage) -> String {
    message
        .payload
        .get("delta")
        .and_then(Value::as_str)
        .or(message.preview.as_deref())
        .unwrap_or_default()
        .to_string()
}

fn is_reasoning_message(message: &DisplayMessage) -> bool {
    message.payload.get("part_kind").and_then(Value::as_str) == Some("thinking")
        || message
            .metadata
            .get("reasoning")
            .and_then(Value::as_bool)
            .unwrap_or(false)
}

fn strip_null_object_fields(value: &mut Value) {
    if let Some(object) = value.as_object_mut() {
        object.retain(|_, value| !value.is_null());
    }
}

pub(super) fn render_display_text(messages: &[DisplayMessage]) -> String {
    let mut output = String::new();
    let mut last_was_text = false;
    for message in messages {
        match message.kind {
            DisplayMessageKind::AssistantTextDelta => {
                if let Some(delta) = message.payload.get("delta").and_then(Value::as_str) {
                    if message.payload.get("part_kind").and_then(Value::as_str) == Some("thinking")
                        || message
                            .metadata
                            .get("reasoning")
                            .and_then(Value::as_bool)
                            .unwrap_or(false)
                    {
                        if last_was_text && !output.ends_with('\n') {
                            output.push('\n');
                        }
                        let _ = writeln!(output, "thinking={delta}");
                        last_was_text = false;
                    } else {
                        output.push_str(delta);
                        last_was_text = true;
                    }
                }
            }
            DisplayMessageKind::ToolCallStart => {
                if last_was_text && !output.ends_with('\n') {
                    output.push('\n');
                }
                let _ = writeln!(
                    output,
                    "tool_call={}",
                    message
                        .payload
                        .get("name")
                        .or_else(|| message.payload.get("tool_name"))
                        .and_then(Value::as_str)
                        .or(message.preview.as_deref())
                        .unwrap_or("tool")
                );
                last_was_text = false;
            }
            DisplayMessageKind::ToolResult => {
                if let Some(preview) = message.preview.as_deref() {
                    let _ = writeln!(output, "tool_result={preview}");
                }
                last_was_text = false;
            }
            DisplayMessageKind::ApprovalRequested => {
                if last_was_text && !output.ends_with('\n') {
                    output.push('\n');
                }
                output.push_str("approval=requested\n");
                last_was_text = false;
            }
            DisplayMessageKind::RunFailed => {
                if last_was_text && !output.ends_with('\n') {
                    output.push('\n');
                }
                let preview = message.preview.as_deref().unwrap_or("run failed");
                let _ = writeln!(output, "status=failed message={preview}");
                last_was_text = false;
            }
            _ => {}
        }
    }
    if last_was_text && !output.ends_with('\n') {
        output.push('\n');
    }
    if output.is_empty()
        && let Some(message) = messages
            .iter()
            .rev()
            .find(|message| message.kind.is_terminal())
    {
        let _ = writeln!(
            output,
            "status={}",
            match message.kind {
                DisplayMessageKind::RunCompleted => "completed",
                DisplayMessageKind::RunFailed => "failed",
                DisplayMessageKind::RunCancelled => "cancelled",
                _ => "unknown",
            }
        );
    }
    output
}

pub(super) fn render_completion(shell: Shell) -> CliResult<String> {
    let mut command = crate::args::command();
    let mut buffer = Vec::new();
    clap_complete::generate(shell, &mut command, "starweaver-cli", &mut buffer);
    String::from_utf8(buffer).map_err(|error| CliError::Run(error.to_string()))
}

pub(super) fn render_approvals(
    approvals: &[ApprovalRecord],
    output: OutputMode,
) -> CliResult<String> {
    match output {
        OutputMode::Text => {
            let mut lines = String::new();
            for approval in approvals {
                let _ = writeln!(
                    lines,
                    "approval_id={} run_id={} action={} status={}",
                    approval.approval_id,
                    approval.run_id.as_str(),
                    approval.action_name,
                    approval_status_name(approval.status)
                );
            }
            Ok(lines)
        }
        OutputMode::DisplayJsonl | OutputMode::AguiJsonl => render_json_lines(approvals),
        OutputMode::Json => Ok(format!(
            "{}\n",
            serde_json::to_string(&json!({"approvals": approvals, "status": "list"}))?
        )),
        OutputMode::Silent => Ok(format!("approvals={}\nstatus=list\n", approvals.len())),
    }
}

pub(super) fn render_deferred(
    records: &[DeferredToolRecord],
    output: OutputMode,
) -> CliResult<String> {
    match output {
        OutputMode::Text => {
            let mut lines = String::new();
            for record in records {
                let _ = writeln!(
                    lines,
                    "deferred_id={} run_id={} tool={} status={}",
                    record.deferred_id,
                    record.run_id.as_str(),
                    record.tool_name,
                    execution_status_name(record.status)
                );
            }
            Ok(lines)
        }
        OutputMode::DisplayJsonl | OutputMode::AguiJsonl => render_json_lines(records),
        OutputMode::Json => Ok(format!(
            "{}\n",
            serde_json::to_string(&json!({"deferred": records, "status": "list"}))?
        )),
        OutputMode::Silent => Ok(format!("deferred={}\nstatus=list\n", records.len())),
    }
}

pub(super) fn render_deferred_decision(
    record: &DeferredToolRecord,
    output: OutputMode,
) -> CliResult<String> {
    match output {
        OutputMode::Text => Ok(format!(
            "deferred_id={}\nstatus={}\nrun_id={}\n",
            record.deferred_id,
            execution_status_name(record.status),
            record.run_id.as_str()
        )),
        OutputMode::DisplayJsonl | OutputMode::AguiJsonl | OutputMode::Json => {
            Ok(format!("{}\n", serde_json::to_string(record)?))
        }
        OutputMode::Silent => Ok(format!(
            "deferred_id={}\nstatus={}\n",
            record.deferred_id,
            execution_status_name(record.status)
        )),
    }
}

pub(super) const fn approval_status_name(status: ApprovalStatus) -> &'static str {
    match status {
        ApprovalStatus::Pending => "pending",
        ApprovalStatus::Approved => "approved",
        ApprovalStatus::Denied => "denied",
        ApprovalStatus::Expired => "expired",
        ApprovalStatus::Cancelled => "cancelled",
    }
}

const fn execution_status_name(status: starweaver_session::ExecutionStatus) -> &'static str {
    match status {
        starweaver_session::ExecutionStatus::Pending => "pending",
        starweaver_session::ExecutionStatus::Running => "running",
        starweaver_session::ExecutionStatus::Waiting => "waiting",
        starweaver_session::ExecutionStatus::Completed => "completed",
        starweaver_session::ExecutionStatus::Failed => "failed",
        starweaver_session::ExecutionStatus::Cancelled => "cancelled",
    }
}

pub(super) fn render_session_delete(
    session_id: &str,
    deleted: bool,
    output: OutputMode,
) -> CliResult<String> {
    match output {
        OutputMode::Text => Ok(format!(
            "session_id={session_id}\ndeleted={deleted}\nstatus=deleted\n"
        )),
        OutputMode::DisplayJsonl | OutputMode::AguiJsonl | OutputMode::Json => Ok(format!(
            "{}\n",
            serde_json::to_string(&json!({
                "session_id": session_id,
                "deleted": deleted,
                "status": "deleted"
            }))?
        )),
        OutputMode::Silent => Ok(format!("session_id={session_id}\nstatus=deleted\n")),
    }
}

pub(super) fn render_trim_report(report: &TrimReport, output: OutputMode) -> CliResult<String> {
    match output {
        OutputMode::Text => Ok(format!(
            "sessions_scanned={} runs_to_trim={} runs_trimmed={} bytes_reclaimed={} dry_run={}\n",
            report.sessions_scanned,
            report.runs_to_trim,
            report.runs_trimmed,
            report.bytes_reclaimed,
            report.dry_run
        )),
        OutputMode::DisplayJsonl | OutputMode::AguiJsonl | OutputMode::Json => {
            Ok(format!("{}\n", serde_json::to_string(report)?))
        }
        OutputMode::Silent => Ok(format!(
            "sessions_scanned={}\nruns_to_trim={}\nruns_trimmed={}\nbytes_reclaimed={}\ndry_run={}\nstatus=trimmed\n",
            report.sessions_scanned,
            report.runs_to_trim,
            report.runs_trimmed,
            report.bytes_reclaimed,
            report.dry_run
        )),
    }
}

pub(super) fn session_value(session: &starweaver_session::SessionRecord) -> Value {
    json!({
        "session_id": session.session_id.as_str(),
        "title": session.title,
        "profile": session.profile,
        "status": format!("{:?}", session.status).to_lowercase(),
        "head_run_id": session.head_run_id.as_ref().map(starweaver_core::RunId::as_str),
        "head_success_run_id": session.head_success_run_id.as_ref().map(starweaver_core::RunId::as_str),
        "active_run_id": session.active_run_id.as_ref().map(starweaver_core::RunId::as_str),
        "created_at": session.created_at.to_rfc3339(),
        "updated_at": session.updated_at.to_rfc3339(),
    })
}
