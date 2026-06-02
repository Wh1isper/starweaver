//! CLI run execution and display augmentation.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use starweaver_agent::{AgentSession, AgentStreamRecord, ResumableState};
use starweaver_environment::DynEnvironmentProvider;
use starweaver_runtime::{AgentStreamEvent, ModelResponseStreamEvent};
use starweaver_session::{
    ApprovalDecision, ApprovalRecord, ApprovalStatus, DeferredToolRecord, ExecutionStatus,
    RunRecord, RunStatus,
};
use starweaver_stream::{
    DefaultDisplayMessageProjector, DisplayMessage, DisplayMessageKind, DisplayMessageProjector,
    DisplayProjectionContext, RealtimeCompactionBuffer, ReplayScope, ReplaySnapshot,
};

use crate::{
    args::HitlPolicy, local_store::RunArtifacts, profiles::ResolvedProfile, CliError, CliResult,
};

/// CLI run policy.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CliRunPolicy {
    /// Headless human-in-the-loop behavior.
    pub hitl: HitlPolicy,
}

/// CLI execution output and durable artifacts.
pub struct CliRunExecution {
    /// Final output preview.
    pub output: String,
    /// Durable artifacts.
    pub artifacts: RunArtifacts,
}

/// Execute a resolved profile through `AgentSession`.
pub fn execute_agent_session(
    prompt: String,
    run: &RunRecord,
    profile: &ResolvedProfile,
    environment: &DynEnvironmentProvider,
    restore_state: Option<ResumableState>,
    policy: CliRunPolicy,
) -> CliResult<CliRunExecution> {
    let agent = profile.build_agent()?;
    let mut session = restore_state.map_or_else(
        || AgentSession::new(agent.clone()),
        |state| AgentSession::from_state(agent.clone(), state),
    );
    session.set_environment(environment.clone());
    session.set_metadata("cli.profile", json!(profile.name));
    if let Some(source) = profile.source.as_ref() {
        session.set_metadata("cli.profile_source", json!(source));
    }
    session.set_metadata("cli.run_id", json!(run.run_id.as_str()));
    let runtime =
        tokio::runtime::Runtime::new().map_err(|error| CliError::Run(error.to_string()))?;
    let stream = runtime
        .block_on(session.run_stream(prompt))
        .map_err(|error| CliError::Run(error.to_string()))?;
    let environment_state = runtime
        .block_on(environment.export_state())
        .map_err(|error| CliError::Run(error.to_string()))?;
    let mut state = session.export_state();
    state
        .metadata
        .insert("cli.run_id".to_string(), json!(run.run_id.as_str()));
    state
        .metadata
        .insert("cli.session_id".to_string(), json!(run.session_id.as_str()));
    let projection = project_display_messages(run, &stream.events, policy, &runtime);
    let artifacts = RunArtifacts {
        state,
        environment_state: Some(environment_state),
        raw_records: stream.events,
        display_messages: projection.messages,
        display_snapshot: projection.snapshot,
        approvals: projection.approvals,
        deferred_tools: projection.deferred_tools,
        status: projection.status,
    };
    Ok(CliRunExecution {
        output: stream.result.output,
        artifacts,
    })
}

struct DisplayProjection {
    messages: Vec<DisplayMessage>,
    snapshot: ReplaySnapshot,
    approvals: Vec<ApprovalRecord>,
    deferred_tools: Vec<DeferredToolRecord>,
    status: RunStatus,
}

#[allow(clippy::too_many_lines)]
fn project_display_messages(
    run: &RunRecord,
    raw_records: &[AgentStreamRecord],
    policy: CliRunPolicy,
    runtime: &tokio::runtime::Runtime,
) -> DisplayProjection {
    let policy_metadata = serde_json::to_value(policy).unwrap_or(Value::Null);
    let context = DisplayProjectionContext::new(run.session_id.clone(), run.run_id.clone());
    let projector = DefaultDisplayMessageProjector;
    let mut display_messages = vec![DisplayMessage::new(
        0,
        run.session_id.clone(),
        run.run_id.clone(),
        DisplayMessageKind::RunQueued,
    )
    .with_payload(json!({"sequence_no": run.sequence_no}))
    .with_preview("run queued")];
    let mut final_result_projected_text = false;
    let mut approvals = Vec::new();
    let mut deferred_tools = Vec::new();
    let mut status = RunStatus::Completed;
    for record in raw_records {
        let projected = runtime.block_on(projector.project(&context, record));
        let projected_has_text_delta = projected
            .iter()
            .any(|message| message.kind == DisplayMessageKind::AssistantTextDelta);
        match &record.event {
            AgentStreamEvent::ModelStream {
                event: ModelResponseStreamEvent::FinalResult(_),
                ..
            } => {
                if projected_has_text_delta {
                    final_result_projected_text = true;
                }
                display_messages.extend(projected);
            }
            AgentStreamEvent::ModelResponse { .. }
                if final_result_projected_text && projected_has_text_delta =>
            {
                final_result_projected_text = false;
            }
            AgentStreamEvent::ModelResponse { response, .. } => {
                final_result_projected_text = false;
                display_messages.extend(projected);
                for part in &response.parts {
                    if let starweaver_model::ModelResponsePart::Thinking { text, signature } = part
                    {
                        let mut message = DisplayMessage::new(
                            record.sequence,
                            run.session_id.clone(),
                            run.run_id.clone(),
                            DisplayMessageKind::AssistantTextDelta,
                        )
                        .with_payload(json!({"thinking": text, "signature": signature}))
                        .with_preview(text.clone());
                        message
                            .metadata
                            .insert("reasoning".to_string(), json!(true));
                        display_messages.push(message);
                    }
                }
            }
            AgentStreamEvent::ToolReturn { tool_return, .. } => {
                display_messages.extend(projected);
                display_messages.push(
                    DisplayMessage::new(
                        record.sequence,
                        run.session_id.clone(),
                        run.run_id.clone(),
                        DisplayMessageKind::ToolCallEnd,
                    )
                    .with_payload(json!({
                        "tool_call_id": tool_return.tool_call_id,
                        "tool_name": tool_return.name,
                        "is_error": tool_return.is_error,
                    }))
                    .with_preview(format!("tool call {} ended", tool_return.name)),
                );
                apply_control_flow(
                    run,
                    tool_return,
                    policy,
                    record.sequence,
                    &mut display_messages,
                    &mut approvals,
                    &mut deferred_tools,
                    &mut status,
                );
            }
            _ => display_messages.extend(projected),
        }
    }
    if status == RunStatus::Failed
        && !display_messages
            .iter()
            .any(|message| message.kind == DisplayMessageKind::RunFailed)
    {
        display_messages.push(
            DisplayMessage::new(
                display_messages.len(),
                run.session_id.clone(),
                run.run_id.clone(),
                DisplayMessageKind::RunFailed,
            )
            .with_payload(json!({"reason": "hitl policy failed"}))
            .with_preview("hitl policy failed"),
        );
    }
    display_messages.push(
        DisplayMessage::new(
            display_messages.len(),
            run.session_id.clone(),
            run.run_id.clone(),
            DisplayMessageKind::CompactionStarted,
        )
        .with_payload(json!({"scope": format!("run:{}", run.run_id.as_str())}))
        .with_preview("display compaction started"),
    );
    resequence_display_messages(&mut display_messages);
    let mut buffer = RealtimeCompactionBuffer::new(ReplayScope::run(run.run_id.as_str()));
    for message in display_messages.clone() {
        buffer.push(message);
    }
    let snapshot = buffer.snapshot();
    display_messages.push(
        DisplayMessage::new(
            display_messages.len(),
            run.session_id.clone(),
            run.run_id.clone(),
            DisplayMessageKind::CompactionCompleted,
        )
        .with_payload(json!({
            "revision": snapshot.revision,
            "messages": snapshot.display_messages.len(),
        }))
        .with_preview("display compaction completed"),
    );
    resequence_display_messages(&mut display_messages);
    for message in &mut display_messages {
        message
            .metadata
            .insert("cli_run_policy".to_string(), policy_metadata.clone());
    }
    DisplayProjection {
        messages: display_messages,
        snapshot,
        approvals,
        deferred_tools,
        status,
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_control_flow(
    run: &RunRecord,
    tool_return: &starweaver_model::ToolReturnPart,
    policy: CliRunPolicy,
    sequence: usize,
    display_messages: &mut Vec<DisplayMessage>,
    approvals: &mut Vec<ApprovalRecord>,
    deferred_tools: &mut Vec<DeferredToolRecord>,
    status: &mut RunStatus,
) {
    let control_flow = tool_return
        .metadata
        .get("control_flow")
        .and_then(Value::as_str);
    match control_flow {
        Some("approval_required") => {
            let approval_id = format!(
                "approval_{}_{}",
                run.run_id.as_str(),
                tool_return.tool_call_id
            );
            let mut record = ApprovalRecord::new(
                approval_id.clone(),
                run.session_id.clone(),
                run.run_id.clone(),
                tool_return.tool_call_id.clone(),
                tool_return.name.clone(),
            );
            record.request = tool_return
                .metadata
                .get("approval")
                .cloned()
                .unwrap_or(Value::Null);
            record
                .metadata
                .insert("policy".to_string(), json!(policy.hitl));
            display_messages.push(
                DisplayMessage::new(
                    sequence,
                    run.session_id.clone(),
                    run.run_id.clone(),
                    DisplayMessageKind::ApprovalRequested,
                )
                .with_payload(json!({
                    "approval_id": approval_id,
                    "tool_call_id": tool_return.tool_call_id,
                    "tool_name": tool_return.name,
                    "request": record.request,
                }))
                .with_preview(format!("approval requested for {}", tool_return.name)),
            );
            apply_approval_policy(run, policy.hitl, &mut record, display_messages, status);
            approvals.push(record);
        }
        Some("call_deferred") => {
            let deferred_id = format!(
                "deferred_{}_{}",
                run.run_id.as_str(),
                tool_return.tool_call_id
            );
            let mut record = DeferredToolRecord::new(
                deferred_id.clone(),
                run.session_id.clone(),
                run.run_id.clone(),
                tool_return.tool_call_id.clone(),
                tool_return.name.clone(),
            );
            record.request = tool_return
                .metadata
                .get("deferred")
                .cloned()
                .unwrap_or(Value::Null);
            record.status = ExecutionStatus::Waiting;
            record
                .metadata
                .insert("policy".to_string(), json!(policy.hitl));
            *status = RunStatus::Waiting;
            display_messages.push(
                DisplayMessage::new(
                    sequence,
                    run.session_id.clone(),
                    run.run_id.clone(),
                    DisplayMessageKind::ApprovalRequested,
                )
                .with_payload(json!({
                    "deferred_id": deferred_id,
                    "tool_call_id": tool_return.tool_call_id,
                    "tool_name": tool_return.name,
                    "request": record.request,
                    "control_flow": "call_deferred",
                }))
                .with_preview(format!("tool call {} deferred", tool_return.name)),
            );
            deferred_tools.push(record);
        }
        _ => {}
    }
}

fn apply_approval_policy(
    run: &RunRecord,
    hitl: HitlPolicy,
    record: &mut ApprovalRecord,
    display_messages: &mut Vec<DisplayMessage>,
    status: &mut RunStatus,
) {
    match hitl {
        HitlPolicy::Deny => {
            record.status = ApprovalStatus::Denied;
            record.decision = Some(decision(ApprovalStatus::Denied, "denied by CLI policy"));
            push_approval_resolved(run, record, display_messages);
        }
        HitlPolicy::Defer | HitlPolicy::Prompt => {
            record.status = ApprovalStatus::Pending;
            *status = RunStatus::Waiting;
        }
        HitlPolicy::Fail => {
            record.status = ApprovalStatus::Denied;
            record.decision = Some(decision(ApprovalStatus::Denied, "failed by CLI policy"));
            *status = RunStatus::Failed;
            push_approval_resolved(run, record, display_messages);
        }
    }
    record.updated_at = Utc::now();
}

fn decision(status: ApprovalStatus, reason: &str) -> ApprovalDecision {
    ApprovalDecision {
        status,
        decided_by: Some("starweaver-cli".to_string()),
        decided_at: Utc::now(),
        reason: Some(reason.to_string()),
        metadata: serde_json::Map::default(),
    }
}

fn push_approval_resolved(
    run: &RunRecord,
    record: &ApprovalRecord,
    display_messages: &mut Vec<DisplayMessage>,
) {
    display_messages.push(
        DisplayMessage::new(
            display_messages.len(),
            run.session_id.clone(),
            run.run_id.clone(),
            DisplayMessageKind::ApprovalResolved,
        )
        .with_payload(json!({
            "approval_id": record.approval_id,
            "status": record.status,
            "decision": record.decision,
        }))
        .with_preview(format!("approval {}", record.approval_id)),
    );
}

fn resequence_display_messages(messages: &mut [DisplayMessage]) {
    for (sequence, message) in messages.iter_mut().enumerate() {
        message.sequence = sequence;
    }
}

/// Build a terminal display message for execution failures.
pub fn failed_display_message(run: &RunRecord, error: &str) -> Vec<DisplayMessage> {
    vec![
        DisplayMessage::new(
            0,
            run.session_id.clone(),
            run.run_id.clone(),
            DisplayMessageKind::RunQueued,
        )
        .with_preview("run queued"),
        DisplayMessage::new(
            1,
            run.session_id.clone(),
            run.run_id.clone(),
            DisplayMessageKind::RunFailed,
        )
        .with_payload(json!({"error": error}))
        .with_preview(error.to_string()),
    ]
}
