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
use starweaver_agent::{AgentSession, AgentStreamRecord, ResumableState};
use starweaver_context::{AgentContext, BusMessage};
use starweaver_environment::DynEnvironmentProvider;
use starweaver_model::{ModelRequest, ModelSettings};
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
    args::HitlPolicy, local_store::RunArtifacts, profiles::ResolvedProfile, CliError, CliResult,
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
    prompt: String,
    run: &RunRecord,
    profile: &ResolvedProfile,
    environment: &DynEnvironmentProvider,
    restore_state: Option<ResumableState>,
    policy: CliRunPolicy,
) -> CliResult<CliRunExecution> {
    execute_agent_session_with_stream_sender(
        prompt,
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
    prompt: String,
    run: &RunRecord,
    profile: &ResolvedProfile,
    environment: &DynEnvironmentProvider,
    restore_state: Option<ResumableState>,
    policy: CliRunPolicy,
    stream_sender: Option<mpsc::Sender<AgentStreamRecord>>,
) -> CliResult<CliRunExecution> {
    execute_agent_session_with_channels(
        prompt,
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
    prompt: String,
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
    let should_observe_stream = stream_sender.is_some() || cancel_receiver.is_some();
    if should_observe_stream {
        agent = agent.with_stream_observer(Arc::new(CliStreamObserver {
            sender: stream_sender,
            records: Arc::clone(&observed_records),
        }));
    }
    if let Some(pending) = pending_steering {
        agent = agent.with_capability(Arc::new(CliSteeringBridge { pending }));
    }
    let mut session = restore_state.map_or_else(
        || AgentSession::new(agent.clone()),
        |state| AgentSession::from_state(agent.clone(), state),
    );
    session.set_environment(environment.clone());
    session.set_metadata("cli.profile", json!(profile.name));
    session.set_metadata("cli.profile_source", json!(profile.source.kind()));
    if let Some(path) = profile.source.path() {
        session.set_metadata("cli.profile_path", json!(path));
    }
    session.set_metadata("cli.run_id", json!(run.run_id.as_str()));
    let runtime =
        tokio::runtime::Runtime::new().map_err(|error| CliError::Run(error.to_string()))?;
    let run_outcome = run_session_stream(&runtime, &mut session, prompt, cancel_receiver)?;
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
    let (output, raw_records, projection) = match run_outcome {
        SessionRunOutcome::Completed(stream) => {
            let projection = project_display_messages(run, &stream.events, policy, &runtime);
            (stream.result.output, stream.events, projection)
        }
        SessionRunOutcome::Cancelled => {
            let raw_records = observed_records
                .lock()
                .map_or_else(|_| Vec::new(), |records| records.clone());
            let projection = cancelled_display_projection(run, &raw_records, policy, &runtime);
            ("cancelled".to_string(), raw_records, projection)
        }
    };
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
                result = run_future => result
                    .map(Box::new)
                    .map(SessionRunOutcome::Completed)
                    .map_err(|error| CliError::Run(error.to_string())),
                () = wait_for_cancel(cancel_receiver) => Ok(SessionRunOutcome::Cancelled),
            }
        })
    } else {
        runtime
            .block_on(run_future)
            .map(Box::new)
            .map(SessionRunOutcome::Completed)
            .map_err(|error| CliError::Run(error.to_string()))
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

#[derive(Default)]
struct PendingSteering {
    messages: Mutex<VecDeque<CliSteeringMessage>>,
}

struct CliSteeringBridge {
    pending: Arc<PendingSteering>,
}

#[async_trait]
impl AgentCapability for CliSteeringBridge {
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
        if drain_pending_steering(&self.pending, context) {
            return Err(starweaver_runtime::CapabilityError::ModelRetry(
                "<system-reminder>There are pending steering messages. Continue and incorporate them before finalizing.</system-reminder>"
                    .to_string(),
            ));
        }
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
            AgentStreamEvent::ModelResponse { .. } => {
                final_result_projected_text = false;
                display_messages.extend(projected);
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

    use starweaver_core::{ConversationId, RunId, SessionId};
    use starweaver_runtime::{AgentStreamEvent, AgentStreamRecord};
    use starweaver_session::{RunRecord, RunStatus};
    use starweaver_stream::DisplayMessageKind;

    use super::{
        cancelled_display_projection, start_steering_collector, CliRunPolicy, CliSteeringMessage,
    };
    use crate::args::HitlPolicy;

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
