use super::{
    json, AgentContext, AgentStreamEvent, AgentStreamRecord, ApprovalDecision, ApprovalRecord,
    ApprovalStatus, CliRunPolicy, DefaultDisplayMessageProjector, DeferredToolRecord,
    DisplayMessage, DisplayMessageKind, DisplayMessageProjector, DisplayProjectionContext,
    FinishReason, HitlPolicy, ModelResponse, ModelResponsePart, ModelResponseStreamEvent,
    RealtimeCompactionBuffer, ReplayScope, ReplaySnapshot, RunRecord, RunStatus, StreamDelta,
    ToolReturnRecordInput, Utc, Value,
};

pub(super) struct DisplayProjection {
    pub(super) messages: Vec<DisplayMessage>,
    pub(super) snapshot: ReplaySnapshot,
    pub(super) approvals: Vec<ApprovalRecord>,
    pub(super) deferred_tools: Vec<DeferredToolRecord>,
    pub(super) status: RunStatus,
}

#[allow(clippy::too_many_lines)]
pub(super) fn project_display_messages(
    run: &RunRecord,
    raw_records: &[AgentStreamRecord],
    policy: &CliRunPolicy,
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
    let mut streamed_text_projected = false;
    let mut streamed_reasoning_projected = false;
    let mut final_result_projected_text = false;
    let mut final_result_projected_reasoning = false;
    let mut approvals = Vec::new();
    let mut deferred_tools = Vec::new();
    let mut status = RunStatus::Completed;
    for record in raw_records {
        let projected = runtime.block_on(projector.project(&context, record));
        let projected_has_text_delta = projected.iter().any(is_visible_assistant_text_delta);
        let projected_has_reasoning_delta = projected.iter().any(is_reasoning_assistant_delta);
        match &record.event {
            AgentStreamEvent::ModelStream {
                event: ModelResponseStreamEvent::PartDelta(_),
                ..
            } => {
                if projected_has_text_delta {
                    streamed_text_projected = true;
                }
                if projected_has_reasoning_delta {
                    streamed_reasoning_projected = true;
                }
                display_messages.extend(projected);
            }
            AgentStreamEvent::ModelStream {
                event: ModelResponseStreamEvent::FinalResult(_),
                ..
            } => {
                if projected_has_text_delta && !streamed_text_projected {
                    final_result_projected_text = true;
                }
                if projected_has_reasoning_delta && !streamed_reasoning_projected {
                    final_result_projected_reasoning = true;
                }
                display_messages.extend(filter_projected_model_messages(
                    projected,
                    streamed_text_projected,
                    streamed_reasoning_projected,
                ));
            }
            AgentStreamEvent::ModelResponse { .. } => {
                let suppress_text = streamed_text_projected || final_result_projected_text;
                let suppress_reasoning =
                    streamed_reasoning_projected || final_result_projected_reasoning;
                streamed_text_projected = false;
                streamed_reasoning_projected = false;
                final_result_projected_text = false;
                final_result_projected_reasoning = false;
                display_messages.extend(filter_projected_model_messages(
                    projected,
                    suppress_text,
                    suppress_reasoning,
                ));
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
    policy: &CliRunPolicy,
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
            let input = ToolReturnRecordInput::new(
                &run.session_id,
                &run.run_id,
                &tool_return.tool_call_id,
                &tool_return.name,
                &tool_return.metadata,
            )
            .with_policy(json!(policy.hitl));
            let Some(mut record) = ApprovalRecord::from_tool_return(&input) else {
                return;
            };
            display_messages.push(
                DisplayMessage::new(
                    sequence,
                    run.session_id.clone(),
                    run.run_id.clone(),
                    DisplayMessageKind::ApprovalRequested,
                )
                .with_payload(json!({
                    "approval_id": record.approval_id.clone(),
                    "tool_call_id": tool_return.tool_call_id,
                    "tool_name": tool_return.name,
                    "request": record.request.clone(),
                }))
                .with_preview(format!("approval requested for {}", tool_return.name)),
            );
            apply_approval_policy(run, policy.hitl, &mut record, display_messages, status);
            approvals.push(record);
        }
        Some("call_deferred") => {
            let input = ToolReturnRecordInput::new(
                &run.session_id,
                &run.run_id,
                &tool_return.tool_call_id,
                &tool_return.name,
                &tool_return.metadata,
            )
            .with_policy(json!(policy.hitl));
            let Some(record) = DeferredToolRecord::from_tool_return(&input) else {
                return;
            };
            *status = RunStatus::Waiting;
            display_messages.push(
                DisplayMessage::new(
                    sequence,
                    run.session_id.clone(),
                    run.run_id.clone(),
                    DisplayMessageKind::ApprovalRequested,
                )
                .with_payload(json!({
                    "deferred_id": &record.deferred_id,
                    "tool_call_id": tool_return.tool_call_id,
                    "tool_name": tool_return.name,
                    "request": &record.request,
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
            *status = RunStatus::Failed;
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

fn filter_projected_model_messages(
    messages: Vec<DisplayMessage>,
    suppress_text: bool,
    suppress_reasoning: bool,
) -> impl Iterator<Item = DisplayMessage> {
    messages.into_iter().filter(move |message| {
        !(is_tool_call_message(message.kind)
            || suppress_text && is_visible_assistant_text_message(message)
            || suppress_reasoning && is_reasoning_assistant_message(message))
    })
}

fn is_visible_assistant_text_delta(message: &DisplayMessage) -> bool {
    message.kind == DisplayMessageKind::AssistantTextDelta
        && !is_reasoning_assistant_message(message)
}

fn is_reasoning_assistant_delta(message: &DisplayMessage) -> bool {
    message.kind == DisplayMessageKind::AssistantTextDelta
        && is_reasoning_assistant_message(message)
}

fn is_visible_assistant_text_message(message: &DisplayMessage) -> bool {
    is_assistant_text_message(message.kind) && !is_reasoning_assistant_message(message)
}

fn is_reasoning_assistant_message(message: &DisplayMessage) -> bool {
    message
        .payload
        .get("part_kind")
        .and_then(serde_json::Value::as_str)
        == Some("thinking")
        || message
            .metadata
            .get("reasoning")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
}

const fn is_assistant_text_message(kind: DisplayMessageKind) -> bool {
    matches!(
        kind,
        DisplayMessageKind::AssistantTextStart
            | DisplayMessageKind::AssistantTextDelta
            | DisplayMessageKind::AssistantTextEnd
    )
}

const fn is_tool_call_message(kind: DisplayMessageKind) -> bool {
    matches!(
        kind,
        DisplayMessageKind::ToolCallStart
            | DisplayMessageKind::ToolCallDelta
            | DisplayMessageKind::ToolCallEnd
    )
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InterruptedPartKind {
    Text,
    Thinking,
    Other,
}

#[derive(Clone, Debug, Default)]
struct InterruptedPartState {
    kind: Option<InterruptedPartKind>,
    content: String,
    ended: bool,
}

pub(super) fn interrupted_partial_response(
    raw_records: &[AgentStreamRecord],
    context: &AgentContext,
) -> Option<ModelResponse> {
    let mut parts = std::collections::BTreeMap::<usize, InterruptedPartState>::new();
    for record in raw_records {
        let AgentStreamEvent::ModelStream { event, .. } = &record.event else {
            continue;
        };
        match event {
            ModelResponseStreamEvent::PartStart(part) => {
                parts.entry(part.index).or_default().kind =
                    Some(interrupted_part_kind(&part.part_kind));
            }
            ModelResponseStreamEvent::PartDelta(delta) => {
                let state = parts.entry(delta.index).or_default();
                match &delta.delta {
                    StreamDelta::Text { text } => {
                        state.kind.get_or_insert(InterruptedPartKind::Text);
                        state.content.push_str(text);
                    }
                    StreamDelta::Thinking { text } => {
                        state.kind.get_or_insert(InterruptedPartKind::Thinking);
                        state.content.push_str(text);
                    }
                    StreamDelta::ToolCallName { .. }
                    | StreamDelta::ToolCallArguments { .. }
                    | StreamDelta::NativePayload { .. }
                    | StreamDelta::FileMetadata { .. } => {
                        state.kind.get_or_insert(InterruptedPartKind::Other);
                    }
                }
            }
            ModelResponseStreamEvent::PartEnd(part) => {
                let state = parts.entry(part.index).or_default();
                if let Some(part_kind) = part.part_kind.as_deref() {
                    state.kind = Some(interrupted_part_kind(part_kind));
                }
                state.ended = true;
            }
            ModelResponseStreamEvent::FinalResult(_) => return None,
        }
    }

    let mut response_parts = Vec::new();
    for (_, part) in parts {
        match part.kind {
            Some(InterruptedPartKind::Text) if !part.content.is_empty() => {
                response_parts.push(ModelResponsePart::Text { text: part.content });
            }
            Some(InterruptedPartKind::Thinking) if part.ended && !part.content.is_empty() => {
                response_parts.push(ModelResponsePart::Thinking {
                    text: part.content,
                    signature: None,
                });
            }
            _ => {}
        }
    }
    if response_parts.is_empty() {
        return None;
    }
    let mut response = ModelResponse {
        parts: response_parts,
        usage: starweaver_usage::Usage::default(),
        model_name: None,
        provider: None,
        finish_reason: Some(FinishReason::Unknown),
        timestamp: Some(Utc::now()),
        run_id: context.run_id.clone(),
        conversation_id: Some(context.conversation_id.clone()),
        metadata: serde_json::Map::default(),
    };
    response
        .metadata
        .insert("starweaver.interrupted.partial".to_string(), json!(true));
    response.metadata.insert(
        "starweaver.interrupted.reason".to_string(),
        json!("stream_interrupted"),
    );
    Some(response)
}

fn interrupted_part_kind(part_kind: &str) -> InterruptedPartKind {
    let normalized = part_kind.to_ascii_lowercase();
    if normalized.contains("thinking") || normalized.contains("reasoning") {
        InterruptedPartKind::Thinking
    } else if normalized.contains("text") || normalized.contains("message") {
        InterruptedPartKind::Text
    } else {
        InterruptedPartKind::Other
    }
}

fn is_internal_display_compaction_marker(message: &DisplayMessage) -> bool {
    matches!(
        message.kind,
        DisplayMessageKind::CompactionStarted | DisplayMessageKind::CompactionCompleted
    ) && (message
        .payload
        .get("scope")
        .and_then(Value::as_str)
        .is_some_and(|scope| scope.starts_with("run:"))
        || message
            .preview
            .as_deref()
            .is_some_and(|preview| preview.starts_with("display compaction ")))
}

pub(super) fn failed_display_projection(
    run: &RunRecord,
    raw_records: &[AgentStreamRecord],
    error: &str,
    policy: &CliRunPolicy,
    runtime: &tokio::runtime::Runtime,
) -> DisplayProjection {
    let mut projection = project_display_messages(run, raw_records, policy, runtime);
    projection
        .messages
        .retain(|message| !is_internal_display_compaction_marker(message));
    append_goal_completion_if_needed(&mut projection.messages, run, policy, "error", Some(error));
    if !projection
        .messages
        .iter()
        .any(|message| message.kind == DisplayMessageKind::RunFailed)
    {
        projection.messages.push(
            DisplayMessage::new(
                projection.messages.len(),
                run.session_id.clone(),
                run.run_id.clone(),
                DisplayMessageKind::RunFailed,
            )
            .with_payload(json!({"error": error}))
            .with_preview(error.to_string()),
        );
    }
    projection.messages.push(
        DisplayMessage::new(
            projection.messages.len(),
            run.session_id.clone(),
            run.run_id.clone(),
            DisplayMessageKind::CompactionStarted,
        )
        .with_payload(json!({"scope": format!("run:{}", run.run_id.as_str())}))
        .with_preview("display compaction started"),
    );
    resequence_display_messages(&mut projection.messages);
    let mut buffer = RealtimeCompactionBuffer::new(ReplayScope::run(run.run_id.as_str()));
    for message in projection.messages.clone() {
        buffer.push(message);
    }
    projection.snapshot = buffer.snapshot();
    projection.messages.push(
        DisplayMessage::new(
            projection.messages.len(),
            run.session_id.clone(),
            run.run_id.clone(),
            DisplayMessageKind::CompactionCompleted,
        )
        .with_payload(json!({
            "revision": projection.snapshot.revision,
            "messages": projection.snapshot.display_messages.len(),
        }))
        .with_preview("display compaction completed"),
    );
    resequence_display_messages(&mut projection.messages);
    projection.status = RunStatus::Failed;
    projection
}

pub(super) fn cancelled_display_projection(
    run: &RunRecord,
    raw_records: &[AgentStreamRecord],
    policy: &CliRunPolicy,
    runtime: &tokio::runtime::Runtime,
) -> DisplayProjection {
    let mut projection = project_display_messages(run, raw_records, policy, runtime);
    projection
        .messages
        .retain(|message| !is_internal_display_compaction_marker(message));
    append_goal_completion_if_needed(&mut projection.messages, run, policy, "cancelled", None);
    projection.messages.push(
        DisplayMessage::new(
            projection.messages.len(),
            run.session_id.clone(),
            run.run_id.clone(),
            DisplayMessageKind::RunCancelled,
        )
        .with_payload(json!({"reason": "cancelled by user"}))
        .with_preview("run cancelled"),
    );
    projection.messages.push(
        DisplayMessage::new(
            projection.messages.len(),
            run.session_id.clone(),
            run.run_id.clone(),
            DisplayMessageKind::CompactionStarted,
        )
        .with_payload(json!({"scope": format!("run:{}", run.run_id.as_str())}))
        .with_preview("display compaction started"),
    );
    resequence_display_messages(&mut projection.messages);
    let mut buffer = RealtimeCompactionBuffer::new(ReplayScope::run(run.run_id.as_str()));
    for message in projection.messages.clone() {
        buffer.push(message);
    }
    projection.snapshot = buffer.snapshot();
    projection.messages.push(
        DisplayMessage::new(
            projection.messages.len(),
            run.session_id.clone(),
            run.run_id.clone(),
            DisplayMessageKind::CompactionCompleted,
        )
        .with_payload(json!({
            "revision": projection.snapshot.revision,
            "messages": projection.snapshot.display_messages.len(),
        }))
        .with_preview("display compaction completed"),
    );
    resequence_display_messages(&mut projection.messages);
    projection.status = RunStatus::Cancelled;
    projection
}

fn append_goal_completion_if_needed(
    messages: &mut Vec<DisplayMessage>,
    run: &RunRecord,
    policy: &CliRunPolicy,
    reason: &str,
    error: Option<&str>,
) {
    let Some(goal) = policy.goal.as_ref() else {
        return;
    };
    if messages
        .iter()
        .any(|message| message.kind == DisplayMessageKind::GoalCompleted)
    {
        return;
    }
    let mut payload = json!({
        "reason": reason,
        "task": goal.objective.as_str(),
        "max_iterations": goal.max_iterations,
    });
    if let Some(error) = error {
        payload["error"] = json!(error);
    }
    messages.push(
        DisplayMessage::new(
            messages.len(),
            run.session_id.clone(),
            run.run_id.clone(),
            DisplayMessageKind::GoalCompleted,
        )
        .with_payload(payload)
        .with_preview(format!("goal completed: {reason}")),
    );
}
