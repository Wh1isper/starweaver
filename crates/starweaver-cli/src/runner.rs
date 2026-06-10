//! CLI run execution and display augmentation.

use std::{
    collections::VecDeque,
    sync::{mpsc, Arc, Mutex},
    thread,
};

use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use starweaver_agent::{
    attach_shell_review_handle, AgentSession, AgentStreamRecord, ResumableState,
};
use starweaver_context::{AgentContext, BusMessage};
use starweaver_environment::DynEnvironmentProvider;
use starweaver_model::{
    ContentPart, FinishReason, ModelMessage, ModelRequest, ModelRequestPart, ModelResponse,
    ModelResponsePart, ModelSettings, StreamDelta,
};
use starweaver_runtime::{
    AgentCapability, AgentRunState, AgentStreamEvent, CapabilityResult, ModelResponseStreamEvent,
};
use starweaver_session::{
    ApprovalDecision, ApprovalRecord, ApprovalStatus, DeferredToolRecord, ExecutionStatus,
    RunRecord, RunStatus,
};
use starweaver_stream::{
    DefaultDisplayMessageProjector, DisplayMessage, DisplayMessageKind, DisplayMessageProjector,
    DisplayProjectionContext, RealtimeCompactionBuffer, ReplayScope, ReplaySnapshot,
};

use crate::{
    args::HitlPolicy, local_store::RunArtifacts, profiles::ResolvedProfile,
    prompt_input::PromptInput, CliError, CliResult,
};

/// CLI run policy.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CliRunPolicy {
    /// Headless human-in-the-loop behavior.
    pub hitl: HitlPolicy,
}

/// Steering message sent from the interactive UI into the running agent.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CliSteeringMessage {
    /// UI-generated stable steering id used to correlate runtime acknowledgements.
    pub id: String,
    /// User steering text.
    pub text: String,
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
    input: PromptInput,
    run: &RunRecord,
    profile: &ResolvedProfile,
    environment: &DynEnvironmentProvider,
    restore_state: Option<ResumableState>,
    policy: CliRunPolicy,
) -> CliResult<CliRunExecution> {
    execute_agent_session_with_stream_sender(
        input,
        run,
        profile,
        environment,
        restore_state,
        policy,
        None,
    )
}

/// Execute a resolved profile and forward live stream records to a caller-owned channel.
pub fn execute_agent_session_with_stream_sender(
    input: PromptInput,
    run: &RunRecord,
    profile: &ResolvedProfile,
    environment: &DynEnvironmentProvider,
    restore_state: Option<ResumableState>,
    policy: CliRunPolicy,
    stream_sender: Option<mpsc::Sender<AgentStreamRecord>>,
) -> CliResult<CliRunExecution> {
    execute_agent_session_with_channels(
        input,
        run,
        profile,
        environment,
        restore_state,
        policy,
        stream_sender,
        None,
        None,
    )
}

/// Execute a resolved profile, forward live stream records, and poll caller-owned steering messages.
#[allow(clippy::too_many_arguments)]
pub fn execute_agent_session_with_channels(
    input: PromptInput,
    run: &RunRecord,
    profile: &ResolvedProfile,
    environment: &DynEnvironmentProvider,
    restore_state: Option<ResumableState>,
    policy: CliRunPolicy,
    stream_sender: Option<mpsc::Sender<AgentStreamRecord>>,
    steering_receiver: Option<mpsc::Receiver<CliSteeringMessage>>,
    cancel_receiver: Option<mpsc::Receiver<()>>,
) -> CliResult<CliRunExecution> {
    let mut agent = profile.build_agent()?;
    let pending_steering = steering_receiver.map(start_steering_collector);
    let observed_records = Arc::new(Mutex::new(Vec::new()));
    agent = agent.with_stream_observer(Arc::new(CliStreamObserver {
        sender: stream_sender,
        records: Arc::clone(&observed_records),
    }));
    let prompt_text = input.text.clone();
    if !input.attachments.is_empty() {
        agent = agent.with_capability(Arc::new(CliPromptAttachmentAdapter {
            content_parts: input.into_content_parts(),
        }));
    }
    if let Some(pending) = pending_steering {
        agent = agent.with_capability(Arc::new(CliSteeringAdapter { pending }));
    }
    let mut session = restore_state.map_or_else(
        || AgentSession::new(agent.clone()),
        |state| AgentSession::from_state(agent.clone(), state),
    );
    session.set_environment(environment.clone());
    profile.configure_context(session.context_mut());
    if let Some(shell_review) = profile.shell_review.as_ref() {
        attach_shell_review_handle(session.context_mut(), shell_review.clone());
    }
    session.set_metadata("cli.profile", json!(profile.name));
    session.set_metadata("cli.profile_source", json!(profile.source.kind()));
    if let Some(path) = profile.source.path() {
        session.set_metadata("cli.profile_path", json!(path));
    }
    session.set_metadata("cli.run_id", json!(run.run_id.as_str()));
    let runtime =
        tokio::runtime::Runtime::new().map_err(|error| CliError::Run(error.to_string()))?;
    let run_outcome = run_session_stream(&runtime, &mut session, prompt_text, cancel_receiver)?;
    let environment_state = runtime
        .block_on(environment.export_state())
        .map_err(|error| CliError::Run(error.to_string()))?;
    let (output, raw_records, projection, saved_interrupted_partial, failure_error) =
        match run_outcome {
            SessionRunOutcome::Completed(stream) => {
                let projection = project_display_messages(run, &stream.events, policy, &runtime);
                (stream.result.output, stream.events, projection, false, None)
            }
            SessionRunOutcome::Cancelled => {
                let raw_records = observed_records
                    .lock()
                    .map_or_else(|_| Vec::new(), |records| records.clone());
                let partial = interrupted_partial_response(&raw_records, session.context());
                let saved_partial = partial.is_some();
                if let Some(partial) = partial {
                    session
                        .context_mut()
                        .message_history
                        .push(ModelMessage::Response(partial));
                }
                let projection = cancelled_display_projection(run, &raw_records, policy, &runtime);
                (
                    "cancelled".to_string(),
                    raw_records,
                    projection,
                    saved_partial,
                    None,
                )
            }
            SessionRunOutcome::Failed(error) => {
                let raw_records = observed_records
                    .lock()
                    .map_or_else(|_| Vec::new(), |records| records.clone());
                let partial = interrupted_partial_response(&raw_records, session.context());
                let saved_partial = partial.is_some();
                if let Some(partial) = partial {
                    session
                        .context_mut()
                        .message_history
                        .push(ModelMessage::Response(partial));
                }
                let projection =
                    failed_display_projection(run, &raw_records, &error, policy, &runtime);
                (
                    error.clone(),
                    raw_records,
                    projection,
                    saved_partial,
                    Some(error),
                )
            }
        };
    let mut state = session.export_state();
    state
        .metadata
        .insert("cli.run_id".to_string(), json!(run.run_id.as_str()));
    state
        .metadata
        .insert("cli.session_id".to_string(), json!(run.session_id.as_str()));
    if projection.status == RunStatus::Cancelled {
        state
            .metadata
            .insert("cli.interrupted".to_string(), json!(true));
        state.metadata.insert(
            "cli.interrupted.reason".to_string(),
            json!("cancelled_by_user"),
        );
        state.metadata.insert(
            "cli.interrupted.saved_partial".to_string(),
            json!(saved_interrupted_partial),
        );
    }
    if let Some(error) = failure_error.as_ref() {
        state.metadata.insert("cli.failed".to_string(), json!(true));
        state
            .metadata
            .insert("cli.failed.error".to_string(), json!(error));
        state.metadata.insert(
            "cli.failed.saved_partial".to_string(),
            json!(saved_interrupted_partial),
        );
    }
    let artifacts = RunArtifacts {
        state,
        environment_state: Some(environment_state),
        raw_records,
        display_messages: projection.messages,
        display_snapshot: projection.snapshot,
        approvals: projection.approvals,
        deferred_tools: projection.deferred_tools,
        status: projection.status,
    };
    Ok(CliRunExecution { output, artifacts })
}

enum SessionRunOutcome {
    Completed(Box<starweaver_agent::AgentStreamResult>),
    Cancelled,
    Failed(String),
}

fn run_session_stream(
    runtime: &tokio::runtime::Runtime,
    session: &mut AgentSession,
    prompt: String,
    cancel_receiver: Option<mpsc::Receiver<()>>,
) -> CliResult<SessionRunOutcome> {
    let run_future = session.run_stream(prompt);
    if let Some(cancel_receiver) = cancel_receiver {
        runtime.block_on(async move {
            tokio::select! {
                result = run_future => Ok(match result {
                    Ok(stream) => SessionRunOutcome::Completed(Box::new(stream)),
                    Err(error) => SessionRunOutcome::Failed(error.to_string()),
                }),
                () = wait_for_cancel(cancel_receiver) => Ok(SessionRunOutcome::Cancelled),
            }
        })
    } else {
        Ok(match runtime.block_on(run_future) {
            Ok(stream) => SessionRunOutcome::Completed(Box::new(stream)),
            Err(error) => SessionRunOutcome::Failed(error.to_string()),
        })
    }
}

async fn wait_for_cancel(cancel_receiver: mpsc::Receiver<()>) {
    loop {
        match cancel_receiver.try_recv() {
            Ok(()) | Err(mpsc::TryRecvError::Disconnected) => return,
            Err(mpsc::TryRecvError::Empty) => {
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
        }
    }
}

fn start_steering_collector(receiver: mpsc::Receiver<CliSteeringMessage>) -> Arc<PendingSteering> {
    let pending = Arc::new(PendingSteering::default());
    let thread_pending = Arc::clone(&pending);
    thread::spawn(move || {
        while let Ok(message) = receiver.recv() {
            if let Ok(mut messages) = thread_pending.messages.lock() {
                messages.push_back(message);
            }
        }
    });
    pending
}

struct CliStreamObserver {
    sender: Option<mpsc::Sender<AgentStreamRecord>>,
    records: Arc<Mutex<Vec<AgentStreamRecord>>>,
}

#[async_trait]
impl AgentCapability for CliStreamObserver {
    async fn on_stream_event(
        &self,
        _state: &AgentRunState,
        event: &AgentStreamRecord,
    ) -> CapabilityResult<()> {
        if let Ok(mut records) = self.records.lock() {
            records.push(event.clone());
        }
        if let Some(sender) = &self.sender {
            let _ = sender.send(event.clone());
        }
        Ok(())
    }
}

struct CliPromptAttachmentAdapter {
    content_parts: Vec<ContentPart>,
}

#[async_trait]
impl AgentCapability for CliPromptAttachmentAdapter {
    async fn before_model_request(
        &self,
        state: &mut AgentRunState,
        request: &mut ModelRequest,
        _settings: &mut Option<ModelSettings>,
    ) -> CapabilityResult<()> {
        if state.run_step != 0 || self.content_parts.is_empty() {
            return Ok(());
        }
        if let Some(ModelRequestPart::UserPrompt {
            content, metadata, ..
        }) = request
            .parts
            .iter_mut()
            .find(|part| matches!(part, ModelRequestPart::UserPrompt { .. }))
        {
            content.clone_from(&self.content_parts);
            metadata.insert(
                "starweaver.cli.attachments".to_string(),
                json!(content
                    .iter()
                    .filter(|part| matches!(part, ContentPart::Binary { .. }))
                    .count()),
            );
        }
        Ok(())
    }
}

#[derive(Default)]
struct PendingSteering {
    messages: Mutex<VecDeque<CliSteeringMessage>>,
}

struct CliSteeringAdapter {
    pending: Arc<PendingSteering>,
}

#[async_trait]
impl AgentCapability for CliSteeringAdapter {
    async fn before_model_request_with_context(
        &self,
        _state: &mut AgentRunState,
        context: &mut AgentContext,
        _request: &mut ModelRequest,
        _settings: &mut Option<ModelSettings>,
    ) -> CapabilityResult<()> {
        drain_pending_steering(&self.pending, context);
        Ok(())
    }

    async fn validate_output_with_context(
        &self,
        _state: &mut AgentRunState,
        context: &mut AgentContext,
        _output: &str,
    ) -> CapabilityResult<()> {
        drain_pending_steering(&self.pending, context);
        Ok(())
    }
}

fn drain_pending_steering(pending: &PendingSteering, context: &mut AgentContext) -> bool {
    let mut drained = false;
    if let Ok(mut messages) = pending.messages.lock() {
        while let Some(message) = messages.pop_front() {
            context.enqueue_message(BusMessage::new(
                "steering",
                json!({"id": message.id, "text": message.text}),
            ));
            drained = true;
        }
    }
    drained
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

fn interrupted_partial_response(
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
        usage: Default::default(),
        model_name: None,
        provider: None,
        finish_reason: Some(FinishReason::Unknown),
        timestamp: Some(Utc::now()),
        run_id: context.run_id.clone(),
        conversation_id: Some(context.conversation_id.clone()),
        metadata: Default::default(),
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

fn failed_display_projection(
    run: &RunRecord,
    raw_records: &[AgentStreamRecord],
    error: &str,
    policy: CliRunPolicy,
    runtime: &tokio::runtime::Runtime,
) -> DisplayProjection {
    let mut projection = project_display_messages(run, raw_records, policy, runtime);
    projection.messages.retain(|message| {
        !matches!(
            message.kind,
            DisplayMessageKind::CompactionStarted | DisplayMessageKind::CompactionCompleted
        )
    });
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

fn cancelled_display_projection(
    run: &RunRecord,
    raw_records: &[AgentStreamRecord],
    policy: CliRunPolicy,
    runtime: &tokio::runtime::Runtime,
) -> DisplayProjection {
    let mut projection = project_display_messages(run, raw_records, policy, runtime);
    projection.messages.retain(|message| {
        !matches!(
            message.kind,
            DisplayMessageKind::CompactionStarted | DisplayMessageKind::CompactionCompleted
        )
    });
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

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use std::{sync::mpsc, thread, time::Duration};

    use starweaver_context::AgentContext;
    use starweaver_core::{ConversationId, RunId, SessionId};
    use starweaver_model::{
        ContentPart, ModelRequest, ModelRequestPart, ModelResponsePart, ModelResponseStreamEvent,
        ModelSettings, PartDelta, PartEnd, PartStart,
    };
    use starweaver_runtime::{AgentCapability, AgentRunState, AgentStreamEvent, AgentStreamRecord};
    use starweaver_session::{RunRecord, RunStatus};
    use starweaver_stream::DisplayMessageKind;

    use super::{
        cancelled_display_projection, interrupted_partial_response, start_steering_collector,
        CliPromptAttachmentAdapter, CliRunPolicy, CliSteeringMessage,
    };
    use crate::{args::HitlPolicy, prompt_input::PromptAttachment};

    #[test]
    fn prompt_attachment_adapter_replaces_initial_user_prompt_with_multimodal_parts() {
        let attachment = PromptAttachment::image(1, b"image-bytes".to_vec(), "image/png");
        let placeholder = attachment.placeholder.clone();
        let adapter = CliPromptAttachmentAdapter {
            content_parts: crate::prompt_input::PromptInput {
                text: format!("inspect this {placeholder} now"),
                attachments: vec![attachment],
            }
            .into_content_parts(),
        };
        let mut state = AgentRunState::new(
            RunId::from_string("run_attach"),
            ConversationId::from_string("conversation_attach"),
        );
        let mut request = ModelRequest::user_text(format!("inspect this {placeholder} now"));
        let mut settings = None::<ModelSettings>;

        tokio::runtime::Runtime::new()
            .expect("runtime should start")
            .block_on(adapter.before_model_request(&mut state, &mut request, &mut settings))
            .expect("adapter should update request");

        let ModelRequestPart::UserPrompt {
            content, metadata, ..
        } = &request.parts[0]
        else {
            panic!("expected user prompt");
        };
        assert_eq!(
            content,
            &vec![
                ContentPart::Text {
                    text: "inspect this  now".to_string(),
                },
                ContentPart::Binary {
                    data: b"image-bytes".to_vec(),
                    media_type: "image/png".to_string(),
                },
            ]
        );
        assert_eq!(metadata["starweaver.cli.attachments"], 1);
    }

    #[test]
    fn prompt_attachment_adapter_only_updates_first_model_step() {
        let adapter = CliPromptAttachmentAdapter {
            content_parts: vec![ContentPart::Binary {
                data: vec![1, 2, 3],
                media_type: "image/png".to_string(),
            }],
        };
        let mut state = AgentRunState::new(
            RunId::from_string("run_attach_skip"),
            ConversationId::from_string("conversation_attach_skip"),
        );
        state.run_step = 1;
        let mut request = ModelRequest::user_text("retry text");
        let mut settings = None::<ModelSettings>;

        tokio::runtime::Runtime::new()
            .expect("runtime should start")
            .block_on(adapter.before_model_request(&mut state, &mut request, &mut settings))
            .expect("adapter should skip request");

        let ModelRequestPart::UserPrompt { content, .. } = &request.parts[0] else {
            panic!("expected user prompt");
        };
        assert_eq!(
            content,
            &vec![ContentPart::Text {
                text: "retry text".to_string(),
            }]
        );
    }

    #[test]
    fn steering_collector_buffers_messages_without_runtime_ack() {
        let (steer_sender, steer_receiver) = mpsc::channel::<CliSteeringMessage>();
        let pending = start_steering_collector(steer_receiver);

        assert!(steer_sender
            .send(CliSteeringMessage {
                id: "steer_test".to_string(),
                text: "tighten scroll".to_string(),
            })
            .is_ok());

        let mut buffered = None;
        for _ in 0..20 {
            buffered = {
                let mut messages = pending
                    .messages
                    .lock()
                    .expect("pending steering lock should be available");
                messages.pop_front()
            };
            if buffered.is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let buffered = buffered.expect("steering message should be buffered");
        assert_eq!(buffered.id, "steer_test");
        assert_eq!(buffered.text, "tighten scroll");
    }

    #[test]
    fn interrupted_partial_response_persists_text_and_completed_thinking_only() {
        let context = AgentContext::default();
        let records = vec![
            AgentStreamRecord::new(
                0,
                AgentStreamEvent::ModelStream {
                    step: 0,
                    event: ModelResponseStreamEvent::PartStart(PartStart {
                        index: 0,
                        part_kind: "text".to_string(),
                    }),
                },
            ),
            AgentStreamRecord::new(
                1,
                AgentStreamEvent::ModelStream {
                    step: 0,
                    event: ModelResponseStreamEvent::PartDelta(PartDelta::text(0, "partial")),
                },
            ),
            AgentStreamRecord::new(
                2,
                AgentStreamEvent::ModelStream {
                    step: 0,
                    event: ModelResponseStreamEvent::PartStart(PartStart {
                        index: 1,
                        part_kind: "thinking".to_string(),
                    }),
                },
            ),
            AgentStreamRecord::new(
                3,
                AgentStreamEvent::ModelStream {
                    step: 0,
                    event: ModelResponseStreamEvent::PartDelta(PartDelta::thinking(
                        1,
                        "done reasoning",
                    )),
                },
            ),
            AgentStreamRecord::new(
                4,
                AgentStreamEvent::ModelStream {
                    step: 0,
                    event: ModelResponseStreamEvent::PartEnd(PartEnd::with_kind(1, "thinking")),
                },
            ),
            AgentStreamRecord::new(
                5,
                AgentStreamEvent::ModelStream {
                    step: 0,
                    event: ModelResponseStreamEvent::PartStart(PartStart {
                        index: 2,
                        part_kind: "thinking".to_string(),
                    }),
                },
            ),
            AgentStreamRecord::new(
                6,
                AgentStreamEvent::ModelStream {
                    step: 0,
                    event: ModelResponseStreamEvent::PartDelta(PartDelta::thinking(
                        2,
                        "unfinished reasoning",
                    )),
                },
            ),
        ];

        let partial = interrupted_partial_response(&records, &context)
            .expect("partial response should be recovered");
        assert_eq!(partial.text_output(), "partial");
        assert_eq!(partial.parts.len(), 2);
        assert_eq!(
            partial.metadata["starweaver.interrupted.partial"],
            serde_json::Value::Bool(true)
        );
        assert!(matches!(
            &partial.parts[1],
            ModelResponsePart::Thinking { text, .. } if text == "done reasoning"
        ));
        assert!(!serde_json::to_string(&partial)
            .unwrap()
            .contains("unfinished reasoning"));
    }

    #[test]
    fn cancelled_projection_preserves_partial_stream_and_terminal_status() {
        let run = RunRecord::new(
            SessionId::from_string("session_cancel"),
            RunId::from_string("run_cancel"),
            ConversationId::from_string("conversation_cancel"),
        );
        let records = vec![AgentStreamRecord::new(
            0,
            AgentStreamEvent::RunStart {
                run_id: RunId::from_string("runtime_run"),
                conversation_id: ConversationId::from_string("conversation_cancel"),
            },
        )];
        let runtime = tokio::runtime::Runtime::new().expect("runtime should start");
        let projection = cancelled_display_projection(
            &run,
            &records,
            CliRunPolicy {
                hitl: HitlPolicy::Deny,
            },
            &runtime,
        );

        assert_eq!(projection.status, RunStatus::Cancelled);
        assert!(projection
            .messages
            .iter()
            .any(|message| message.kind == DisplayMessageKind::RunStarted));
        assert!(projection
            .messages
            .iter()
            .any(|message| message.kind == DisplayMessageKind::RunCancelled));
        assert_eq!(
            projection
                .messages
                .iter()
                .filter(|message| message.kind == DisplayMessageKind::CompactionCompleted)
                .count(),
            1
        );
    }
}
