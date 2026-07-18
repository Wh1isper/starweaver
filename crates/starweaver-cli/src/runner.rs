//! CLI run execution and display augmentation.

use std::{
    collections::{HashSet, VecDeque},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::Duration,
};

use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use starweaver_agent::{
    AgentError, AgentSession, AgentStreamRecord, BackgroundSubagentSupervisor, ResumableState,
    SubagentDelegationMode, attach_process_shell, attach_shell_review_handle,
};
use starweaver_context::{AgentContext, BusMessage};
use starweaver_core::{CancellationToken, SessionId};
use starweaver_environment::{
    DynEnvironmentProvider, DynProcessShellProvider, EnvironmentError, EnvironmentState,
};
use starweaver_model::{
    ContentPart, FinishReason, INSTRUCTION_DYNAMIC_METADATA, INSTRUCTION_ORIGIN_METADATA,
    ModelMessage, ModelRequest, ModelRequestPart, ModelResponse, ModelResponsePart, ModelSettings,
    StreamDelta,
};
use starweaver_runtime::{
    AgentCapability, AgentRunState, AgentStreamEvent, CapabilityResult, GoalCapability,
    GoalRunOptions, ModelResponseStreamEvent, OutputPolicy,
};
use starweaver_session::{
    ApprovalDecision, ApprovalRecord, ApprovalStatus, DeferredToolRecord, RunRecord, RunStatus,
    ToolReturnRecordInput,
};
use starweaver_stream::{
    DefaultDisplayMessageProjector, DisplayMessage, DisplayMessageKind, DisplayMessageProjector,
    DisplayProjectionContext, RealtimeCompactionBuffer, ReplayScope, ReplaySnapshot,
};

use crate::{
    CliError, CliResult, args::HitlPolicy, local_store::RunArtifacts, profiles::ResolvedProfile,
    prompt_input::PromptInput,
};

mod projection;

pub use projection::failed_display_message;
use projection::{
    cancelled_display_projection, failed_display_projection, interrupted_partial_response,
    project_display_messages,
};

/// CLI run policy.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CliRunPolicy {
    /// Headless human-in-the-loop behavior.
    pub hitl: HitlPolicy,
    /// Runtime goal-mode behavior.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<CliGoalRunPolicy>,
}

/// CLI goal-mode policy forwarded to the runtime agent.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CliGoalRunPolicy {
    /// Goal objective.
    pub objective: String,
    /// Maximum runtime goal retry iterations.
    pub max_iterations: usize,
}

/// Steering message sent from the interactive UI into the running agent.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CliSteeringMessage {
    /// UI-generated stable steering id used to correlate runtime acknowledgements.
    pub id: String,
    /// User steering text.
    pub text: String,
}

/// Shared steering admission queue for one interactive run.
#[derive(Clone)]
pub struct CliSteeringChannel {
    state: Arc<Mutex<CliSteeringState>>,
}

#[derive(Default)]
struct CliSteeringState {
    accepting: bool,
    pending: VecDeque<CliSteeringMessage>,
}

impl Default for CliSteeringChannel {
    fn default() -> Self {
        Self::new()
    }
}

impl CliSteeringChannel {
    pub(crate) fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(CliSteeringState {
                accepting: true,
                pending: VecDeque::new(),
            })),
        }
    }

    pub(crate) fn submit(&self, message: CliSteeringMessage) -> Result<(), CliSteeringMessage> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.accepting {
            return Err(message);
        }
        state.pending.push_back(message);
        drop(state);
        Ok(())
    }

    fn drain_into(&self, context: &mut AgentContext, seal_if_empty: bool) -> bool {
        let messages = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.pending.is_empty() && seal_if_empty {
                state.accepting = false;
            }
            state.pending.drain(..).collect::<Vec<_>>()
        };
        let drained = !messages.is_empty();
        for message in messages {
            context.enqueue_message(BusMessage::new(
                "steering",
                json!({"id": message.id, "text": message.text}),
            ));
        }
        drained
    }
}

#[derive(Clone)]
pub struct CliAgentExecutionHost {
    delegation_mode: SubagentDelegationMode,
    background_subagent_supervisor: Option<Arc<BackgroundSubagentSupervisor>>,
    runtime: Option<Arc<tokio::runtime::Runtime>>,
}

impl CliAgentExecutionHost {
    pub const fn blocking() -> Self {
        Self {
            delegation_mode: SubagentDelegationMode::Blocking,
            background_subagent_supervisor: None,
            runtime: None,
        }
    }

    pub const fn disabled() -> Self {
        Self {
            delegation_mode: SubagentDelegationMode::Disabled,
            background_subagent_supervisor: None,
            runtime: None,
        }
    }

    pub const fn interactive(
        supervisor: Arc<BackgroundSubagentSupervisor>,
        runtime: Arc<tokio::runtime::Runtime>,
    ) -> Self {
        Self {
            delegation_mode: SubagentDelegationMode::Async,
            background_subagent_supervisor: Some(supervisor),
            runtime: Some(runtime),
        }
    }
}

impl Default for CliAgentExecutionHost {
    fn default() -> Self {
        Self::blocking()
    }
}

/// CLI execution output and durable artifacts.
pub struct CliRunExecution {
    /// Final output preview.
    pub output: String,
    /// Durable artifacts.
    pub artifacts: RunArtifacts,
}

/// Execute a resolved profile through `AgentSession`.
#[allow(dead_code)]
pub fn execute_agent_session(
    input: PromptInput,
    run: &RunRecord,
    profile: &ResolvedProfile,
    environment: &DynEnvironmentProvider,
    process_environment: Option<&DynProcessShellProvider>,
    restore_state: Option<ResumableState>,
    policy: &CliRunPolicy,
) -> CliResult<CliRunExecution> {
    execute_agent_session_with_stream_sender(
        input,
        run,
        profile,
        environment,
        process_environment,
        restore_state,
        policy,
        None,
    )
}

/// Execute a resolved profile and forward live stream records to a caller-owned channel.
#[allow(dead_code, clippy::too_many_arguments)]
pub fn execute_agent_session_with_stream_sender(
    input: PromptInput,
    run: &RunRecord,
    profile: &ResolvedProfile,
    environment: &DynEnvironmentProvider,
    process_environment: Option<&DynProcessShellProvider>,
    restore_state: Option<ResumableState>,
    policy: &CliRunPolicy,
    stream_sender: Option<mpsc::SyncSender<AgentStreamRecord>>,
) -> CliResult<CliRunExecution> {
    execute_agent_session_with_channels(
        input,
        run,
        profile,
        environment,
        process_environment,
        restore_state,
        policy,
        stream_sender,
        None,
        None,
    )
}

/// Execute a resolved profile, forward live stream records, and poll caller-owned steering messages.
#[allow(dead_code, clippy::too_many_arguments)]
pub fn execute_agent_session_with_channels(
    input: PromptInput,
    run: &RunRecord,
    profile: &ResolvedProfile,
    environment: &DynEnvironmentProvider,
    process_environment: Option<&DynProcessShellProvider>,
    restore_state: Option<ResumableState>,
    policy: &CliRunPolicy,
    stream_sender: Option<mpsc::SyncSender<AgentStreamRecord>>,
    steering_channel: Option<CliSteeringChannel>,
    cancel_receiver: Option<mpsc::Receiver<()>>,
) -> CliResult<CliRunExecution> {
    execute_agent_session_with_host(
        input,
        run,
        profile,
        environment,
        process_environment,
        restore_state,
        policy,
        stream_sender,
        steering_channel,
        cancel_receiver,
        None,
        CliAgentExecutionHost::blocking(),
    )
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub fn execute_agent_session_with_host(
    input: PromptInput,
    run: &RunRecord,
    profile: &ResolvedProfile,
    environment: &DynEnvironmentProvider,
    process_environment: Option<&DynProcessShellProvider>,
    restore_state: Option<ResumableState>,
    policy: &CliRunPolicy,
    stream_sender: Option<mpsc::SyncSender<AgentStreamRecord>>,
    steering_channel: Option<CliSteeringChannel>,
    cancel_receiver: Option<mpsc::Receiver<()>>,
    admission_cancel_receiver: Option<mpsc::Receiver<()>>,
    host: CliAgentExecutionHost,
) -> CliResult<CliRunExecution> {
    let mut agent = profile.build_agent_with_delegation(
        host.delegation_mode,
        host.background_subagent_supervisor.clone(),
    )?;
    let cancellation_token = (cancel_receiver.is_some() || admission_cancel_receiver.is_some())
        .then(CancellationToken::new);
    if let Some(goal) = policy.goal.as_ref() {
        let options = GoalRunOptions::new(goal.objective.clone(), goal.max_iterations);
        let retry_budget = options.max_iterations().saturating_add(5);
        agent = agent
            .with_output_policy(OutputPolicy::new().with_retries(retry_budget))
            .with_capability(Arc::new(GoalCapability::new(options)));
    }
    let observed_records = Arc::new(Mutex::new(Vec::new()));
    agent = agent.with_stream_observer(Arc::new(CliStreamObserver {
        sender: stream_sender,
        records: Arc::clone(&observed_records),
    }));
    let prompt_text = input.text.clone();
    let guidance_text_parts = input.guidance_text_parts.clone();
    if input.has_content_parts() {
        agent = agent.with_capability(Arc::new(CliPromptContentAdapter {
            content_parts: input.into_content_parts(),
        }));
    }
    agent = agent.with_capability(Arc::new(CliGuidanceAdapter {
        guidance_text_parts,
    }));
    if let Some(channel) = steering_channel {
        agent = agent.with_capability(Arc::new(CliSteeringAdapter { channel }));
    }
    if let Some(token) = cancellation_token.as_ref() {
        agent = agent.with_cancellation_token(token.clone());
    }
    let mut session = restore_state.map_or_else(
        || AgentSession::new(agent.clone()),
        |state| AgentSession::from_state(agent.clone(), state),
    );
    session.set_environment(environment.clone());
    if let Some(process_environment) = process_environment {
        attach_process_shell(session.context_mut(), process_environment.clone());
    }
    profile.configure_context(session.context_mut());
    if let Some(shell_review) = profile.shell_review.as_ref() {
        attach_shell_review_handle(session.context_mut(), shell_review.clone());
    }
    sync_run_execution_identity(&mut session, run);
    session.set_metadata("cli.profile", json!(profile.name));
    session.set_metadata("cli.profile_source", json!(profile.source.kind()));
    if let Some(path) = profile.source.path() {
        session.set_metadata("cli.profile_path", json!(path));
    }
    session.set_metadata("cli.run_id", json!(run.run_id.as_str()));
    let runtime = match host.runtime {
        Some(runtime) => runtime,
        None => Arc::new(
            tokio::runtime::Runtime::new().map_err(|error| CliError::Run(error.to_string()))?,
        ),
    };
    if let Some(supervisor) = host.background_subagent_supervisor.as_ref() {
        supervisor.begin_parent_run(run.run_id.clone());
    }
    let cancel_receivers = [cancel_receiver, admission_cancel_receiver]
        .into_iter()
        .flatten()
        .collect();
    let run_outcome = run_session_stream(
        &runtime,
        &mut session,
        prompt_text,
        cancel_receivers,
        cancellation_token,
    );
    if let Some(supervisor) = host.background_subagent_supervisor.as_ref() {
        supervisor.end_parent_run(&run.run_id);
    }
    let run_outcome = run_outcome?;
    let environment_state = runtime.block_on(environment.export_state());
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
    let mut state = session.export_full_state();
    let environment_state = preserve_environment_export_evidence(&mut state, environment_state);
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
        environment_state,
        raw_records,
        display_messages: projection.messages,
        display_snapshot: projection.snapshot,
        approvals: projection.approvals,
        deferred_tools: projection.deferred_tools,
        status: projection.status,
    };
    Ok(CliRunExecution { output, artifacts })
}

fn preserve_environment_export_evidence(
    state: &mut ResumableState,
    environment_state: Result<EnvironmentState, EnvironmentError>,
) -> Option<EnvironmentState> {
    match environment_state {
        Ok(environment_state) => Some(environment_state),
        Err(error) => {
            state
                .metadata
                .insert("cli.environment_export_failed".to_string(), json!(true));
            state.metadata.insert(
                "cli.environment_export_error".to_string(),
                json!(error.to_string()),
            );
            None
        }
    }
}

enum SessionRunOutcome {
    Completed(Box<starweaver_agent::AgentStreamResult>),
    Cancelled,
    Failed(String),
}

fn session_run_outcome(
    result: Result<starweaver_agent::AgentStreamResult, AgentError>,
) -> SessionRunOutcome {
    match result {
        Ok(stream) => SessionRunOutcome::Completed(Box::new(stream)),
        Err(AgentError::Cancelled { .. }) => SessionRunOutcome::Cancelled,
        Err(error) => SessionRunOutcome::Failed(error.to_string()),
    }
}

fn sync_run_execution_identity(session: &mut AgentSession, run: &RunRecord) {
    sync_run_request_metadata(session, run);
    sync_run_session_affinity(session, run);
    session.context_mut().conversation_id = run.conversation_id.clone();
}

fn sync_run_request_metadata(session: &mut AgentSession, run: &RunRecord) {
    session.set_metadata(
        "starweaver.durable_session_id",
        json!(run.session_id.as_str()),
    );
    session.set_metadata("starweaver.durable_run_id", json!(run.run_id.as_str()));
    session.set_metadata("cli.session_id", json!(run.session_id.as_str()));
    session.set_metadata("cli.run_id", json!(run.run_id.as_str()));
    if let Some(skills) = run.metadata.get("cli.skills.activated") {
        session.set_metadata("starweaver.skills.activated", skills.clone());
    } else {
        session
            .context_mut()
            .metadata
            .remove("starweaver.skills.activated");
    }
}

fn sync_run_session_affinity(session: &mut AgentSession, run: &RunRecord) {
    if let Some(affinity_id) = run
        .metadata
        .get("starweaver.session_affinity_id")
        .and_then(Value::as_str)
        .map(SessionId::from_string)
    {
        session.set_session_id(affinity_id);
    } else if session.context().session_id().is_none() {
        session.set_session_id(run.session_id.clone());
    }
}

fn run_session_stream(
    runtime: &tokio::runtime::Runtime,
    session: &mut AgentSession,
    prompt: String,
    cancel_receivers: Vec<mpsc::Receiver<()>>,
    cancellation_token: Option<CancellationToken>,
) -> CliResult<SessionRunOutcome> {
    let run_future = session.run_stream(prompt);
    if cancel_receivers.is_empty() {
        Ok(session_run_outcome(runtime.block_on(run_future)))
    } else {
        let cancellation_token = cancellation_token.unwrap_or_default();
        let cancel_watchers = cancel_receivers
            .into_iter()
            .map(|receiver| spawn_cancel_watcher(receiver, cancellation_token.clone()))
            .collect::<Vec<_>>();
        runtime.block_on(async move {
            tokio::pin!(run_future);
            let outcome = tokio::select! {
                result = &mut run_future => session_run_outcome(result),
                () = cancellation_token.cancelled() => SessionRunOutcome::Cancelled,
            };
            for watcher in cancel_watchers {
                watcher.complete();
            }
            Ok(outcome)
        })
    }
}

struct CancelWatcher {
    completion_sender: mpsc::Sender<()>,
}

impl CancelWatcher {
    fn complete(self) {
        let _ = self.completion_sender.send(());
    }
}

fn spawn_cancel_watcher(
    cancel_receiver: mpsc::Receiver<()>,
    cancellation_token: CancellationToken,
) -> CancelWatcher {
    let (completion_sender, completion_receiver) = mpsc::channel();
    thread::spawn(move || {
        loop {
            if completion_receiver.try_recv().is_ok() {
                return;
            }
            match cancel_receiver.recv_timeout(Duration::from_millis(25)) {
                Ok(()) => {
                    cancellation_token.cancel();
                    return;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }
    });
    CancelWatcher { completion_sender }
}

struct CliStreamObserver {
    sender: Option<mpsc::SyncSender<AgentStreamRecord>>,
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

struct CliPromptContentAdapter {
    content_parts: Vec<ContentPart>,
}

const CLI_GUIDANCE_ORIGIN: &str = "cli_guidance";
const CLI_GUIDANCE_KEY_METADATA: &str = "starweaver.cli.guidance_key";

struct CliGuidanceAdapter {
    guidance_text_parts: Vec<String>,
}

#[async_trait]
impl AgentCapability for CliPromptContentAdapter {
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
                json!(
                    content
                        .iter()
                        .filter(|part| matches!(part, ContentPart::Binary { .. }))
                        .count()
                ),
            );
        }
        Ok(())
    }
}

#[async_trait]
impl AgentCapability for CliGuidanceAdapter {
    async fn prepare_model_messages_with_context(
        &self,
        _state: &mut AgentRunState,
        _context: &mut AgentContext,
        mut messages: Vec<ModelMessage>,
    ) -> CapabilityResult<Vec<ModelMessage>> {
        let current_guidance = current_guidance_parts(&self.guidance_text_parts);
        sync_cli_guidance_history(&mut messages, &current_guidance);
        let parts = current_guidance
            .iter()
            .filter(|guidance| !messages_contain_guidance_key(&messages, &guidance.key))
            .map(|guidance| {
                let mut metadata = serde_json::Map::new();
                metadata.insert(
                    INSTRUCTION_ORIGIN_METADATA.to_string(),
                    json!(CLI_GUIDANCE_ORIGIN),
                );
                metadata.insert(CLI_GUIDANCE_KEY_METADATA.to_string(), json!(guidance.key));
                ModelRequestPart::SystemPrompt {
                    text: guidance.text.clone(),
                    metadata,
                }
            })
            .collect::<Vec<_>>();
        if parts.is_empty() {
            return Ok(messages);
        }
        let Some(ModelMessage::Request(request)) = messages
            .iter_mut()
            .rev()
            .find(|message| matches!(message, ModelMessage::Request(_)))
        else {
            messages.push(ModelMessage::Request(ModelRequest {
                parts,
                timestamp: None,
                instructions: None,
                run_id: None,
                conversation_id: None,
                metadata: serde_json::Map::new(),
            }));
            return Ok(messages);
        };
        let insert_at = guidance_insert_index(request);
        request.parts.splice(insert_at..insert_at, parts);
        Ok(messages)
    }
}

struct GuidancePart {
    key: String,
    text: String,
}

fn current_guidance_parts(guidance_text_parts: &[String]) -> Vec<GuidancePart> {
    let mut seen_keys = HashSet::new();
    guidance_text_parts
        .iter()
        .filter(|text| !text.trim().is_empty())
        .map(|text| GuidancePart {
            key: guidance_key(text),
            text: text.clone(),
        })
        .filter(|guidance| seen_keys.insert(guidance.key.clone()))
        .collect()
}

fn sync_cli_guidance_history(messages: &mut [ModelMessage], current_guidance: &[GuidancePart]) {
    let latest_request_index = messages
        .iter()
        .rposition(|message| matches!(message, ModelMessage::Request(_)));
    let mut retained_keys = HashSet::new();
    for (index, message) in messages.iter_mut().enumerate() {
        let ModelMessage::Request(request) = message else {
            continue;
        };
        for part in &mut request.parts {
            let Some(existing_key) = cli_guidance_key(part) else {
                continue;
            };
            let Some(guidance) = current_guidance
                .iter()
                .find(|guidance| guidance.key == existing_key)
            else {
                continue;
            };
            replace_cli_guidance_part(part, guidance);
        }
        request.parts.retain(|part| {
            cli_guidance_key(part).is_none_or(|existing_key| {
                let is_current = current_guidance.iter().any(|guidance| {
                    guidance.key == existing_key && guidance.text == part_text(part)
                });
                is_current
                    && Some(index) == latest_request_index
                    && retained_keys.insert(existing_key)
            })
        });
    }
}

fn messages_contain_guidance_key(messages: &[ModelMessage], key: &str) -> bool {
    messages.iter().any(|message| match message {
        ModelMessage::Request(request) => request
            .parts
            .iter()
            .any(|part| cli_guidance_key(part).is_some_and(|existing_key| existing_key == key)),
        ModelMessage::Response(_) => false,
    })
}

fn replace_cli_guidance_part(part: &mut ModelRequestPart, guidance: &GuidancePart) {
    let mut metadata = serde_json::Map::new();
    metadata.insert(
        INSTRUCTION_ORIGIN_METADATA.to_string(),
        json!(CLI_GUIDANCE_ORIGIN),
    );
    metadata.insert(CLI_GUIDANCE_KEY_METADATA.to_string(), json!(guidance.key));
    *part = ModelRequestPart::SystemPrompt {
        text: guidance.text.clone(),
        metadata,
    };
}

fn cli_guidance_key(part: &ModelRequestPart) -> Option<String> {
    let metadata = match part {
        ModelRequestPart::SystemPrompt { metadata, .. }
        | ModelRequestPart::Instruction { metadata, .. }
        | ModelRequestPart::UserPrompt { metadata, .. } => metadata,
        ModelRequestPart::ToolReturn(_) | ModelRequestPart::RetryPrompt { .. } => return None,
    };
    if metadata.get(INSTRUCTION_ORIGIN_METADATA) != Some(&serde_json::json!(CLI_GUIDANCE_ORIGIN)) {
        return None;
    }
    metadata
        .get(CLI_GUIDANCE_KEY_METADATA)
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string)
        .or_else(|| Some(guidance_key(part_text(part))))
}

fn part_text(part: &ModelRequestPart) -> &str {
    match part {
        ModelRequestPart::SystemPrompt { text, .. }
        | ModelRequestPart::Instruction { text, .. } => text,
        ModelRequestPart::UserPrompt { content, .. } => content
            .iter()
            .find_map(|part| match part {
                ContentPart::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .unwrap_or_default(),
        ModelRequestPart::ToolReturn(_) | ModelRequestPart::RetryPrompt { .. } => "",
    }
}

fn guidance_key(text: &str) -> String {
    let Some(tag_start) = text.strip_prefix('<') else {
        return text.to_string();
    };
    let Some(tag_end) = tag_start.find('>') else {
        return text.to_string();
    };
    tag_start[..tag_end].to_string()
}

fn guidance_insert_index(request: &ModelRequest) -> usize {
    let control_prefix_len = request
        .parts
        .iter()
        .take_while(|part| is_control_prefix_part(part))
        .count();
    control_prefix_len
        + request.parts[control_prefix_len..]
            .iter()
            .take_while(|part| is_static_instruction_prefix_part(part))
            .count()
}

fn is_control_prefix_part(part: &ModelRequestPart) -> bool {
    match part {
        ModelRequestPart::ToolReturn(_) | ModelRequestPart::RetryPrompt { .. } => true,
        ModelRequestPart::UserPrompt { metadata, .. } => metadata
            .get(INSTRUCTION_ORIGIN_METADATA)
            .and_then(serde_json::Value::as_str)
            .is_some_and(|origin| origin == "tool_return_media"),
        ModelRequestPart::SystemPrompt { .. } | ModelRequestPart::Instruction { .. } => false,
    }
}

fn is_static_instruction_prefix_part(part: &ModelRequestPart) -> bool {
    match part {
        ModelRequestPart::SystemPrompt { .. } => true,
        ModelRequestPart::Instruction { metadata, .. } => !metadata
            .get(INSTRUCTION_DYNAMIC_METADATA)
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        ModelRequestPart::UserPrompt { .. }
        | ModelRequestPart::ToolReturn(_)
        | ModelRequestPart::RetryPrompt { .. } => false,
    }
}

struct CliSteeringAdapter {
    channel: CliSteeringChannel,
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
        self.channel.drain_into(context, false);
        Ok(())
    }

    async fn validate_output_with_context(
        &self,
        _state: &mut AgentRunState,
        context: &mut AgentContext,
        _output: &str,
    ) -> CapabilityResult<()> {
        self.channel.drain_into(context, true);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]
    use std::{
        sync::{Arc, Mutex, mpsc},
        thread,
        time::Duration,
    };

    use starweaver_agent::{
        AgentBuilder, AgentSession, FunctionModel, FunctionModelInfo, ResumableState,
    };
    use starweaver_context::AgentContext;
    use starweaver_core::{CancellationToken, ConversationId, RunId, SessionId};
    use starweaver_model::{
        CONTEXT_ORIGIN_METADATA, CONTEXT_ORIGIN_RUNTIME_CONTEXT, ContentPart,
        INSTRUCTION_DYNAMIC_METADATA, INSTRUCTION_ORIGIN_METADATA, ModelAdapter, ModelError,
        ModelMessage, ModelProfile, ModelRequest, ModelRequestContext, ModelRequestParameters,
        ModelRequestPart, ModelResponse, ModelResponseEventStream, ModelResponsePart,
        ModelResponseStreamEvent, ModelSettings, PartDelta, PartEnd, PartStart, ProtocolFamily,
        providers::openai_responses::OpenAiResponsesAdapter,
    };
    use starweaver_runtime::{AgentCapability, AgentRunState, AgentStreamEvent, AgentStreamRecord};
    use starweaver_session::{RunRecord, RunStatus};
    use starweaver_stream::DisplayMessageKind;

    use super::{
        CLI_GUIDANCE_KEY_METADATA, CLI_GUIDANCE_ORIGIN, CliGuidanceAdapter,
        CliPromptContentAdapter, CliRunPolicy, CliSteeringAdapter, CliSteeringChannel,
        CliSteeringMessage, SessionRunOutcome, cancelled_display_projection, cli_guidance_key,
        interrupted_partial_response, preserve_environment_export_evidence, run_session_stream,
        sync_run_execution_identity, sync_run_request_metadata, sync_run_session_affinity,
    };
    use crate::{args::HitlPolicy, prompt_input::PromptAttachment};

    #[test]
    fn environment_export_failure_is_recorded_without_discarding_session_state() {
        let mut state = ResumableState::default();
        state
            .message_history
            .push(ModelMessage::Request(ModelRequest::user_text(
                "retained prompt",
            )));

        let environment_state = preserve_environment_export_evidence(
            &mut state,
            Err(starweaver_environment::EnvironmentError::Provider(
                "snapshot unavailable".to_string(),
            )),
        );

        assert!(environment_state.is_none());
        assert_eq!(state.message_history.len(), 1);
        assert_eq!(state.metadata["cli.environment_export_failed"], true);
        assert!(
            state.metadata["cli.environment_export_error"]
                .as_str()
                .is_some_and(|error| error.contains("snapshot unavailable"))
        );
    }

    #[test]
    fn sync_run_execution_identity_adopts_durable_conversation() {
        let agent = AgentBuilder::new(Arc::new(FunctionModel::new(
            |_messages: Vec<ModelMessage>,
             _settings: Option<ModelSettings>,
             _info: FunctionModelInfo| { Ok(ModelResponse::text("ok")) },
        )))
        .build();
        let mut session = AgentSession::new(agent);
        let run = RunRecord::new(
            SessionId::from_string("session_execution_identity"),
            RunId::from_string("run_execution_identity"),
            ConversationId::from_string("conversation_execution_identity"),
        );

        sync_run_execution_identity(&mut session, &run);

        assert_eq!(session.context().conversation_id, run.conversation_id);
        assert_eq!(
            session.context().metadata["starweaver.durable_run_id"],
            "run_execution_identity"
        );
    }

    #[test]
    fn sync_run_request_metadata_sets_durable_session_and_run_metadata() {
        let agent = AgentBuilder::new(Arc::new(FunctionModel::new(
            |_messages: Vec<ModelMessage>,
             _settings: Option<ModelSettings>,
             _info: FunctionModelInfo| { Ok(ModelResponse::text("ok")) },
        )))
        .build();
        let mut session = AgentSession::new(agent);
        let run = RunRecord::new(
            SessionId::from_string("session_cli_header"),
            RunId::from_string("run_cli_header"),
            ConversationId::from_string("conversation_cli_header"),
        );

        sync_run_request_metadata(&mut session, &run);

        assert_eq!(
            session.context().metadata["starweaver.durable_session_id"],
            "session_cli_header"
        );
        assert_eq!(
            session.context().metadata["starweaver.durable_run_id"],
            "run_cli_header"
        );
        assert_eq!(
            session.context().metadata["cli.session_id"],
            "session_cli_header"
        );
        assert_eq!(session.context().metadata["cli.run_id"], "run_cli_header");
    }

    #[test]
    fn sync_run_session_affinity_prefers_explicit_affinity_metadata() {
        let agent = AgentBuilder::new(Arc::new(FunctionModel::new(
            |_messages: Vec<ModelMessage>,
             _settings: Option<ModelSettings>,
             _info: FunctionModelInfo| { Ok(ModelResponse::text("ok")) },
        )))
        .build();
        let mut session = AgentSession::new(agent);
        let mut run = RunRecord::new(
            SessionId::from_string("session_durable"),
            RunId::from_string("run_affinity"),
            ConversationId::from_string("conversation_affinity"),
        );
        run.metadata.insert(
            "starweaver.session_affinity_id".to_string(),
            serde_json::json!("session_process_affinity"),
        );

        sync_run_session_affinity(&mut session, &run);

        assert_eq!(
            session.context().session_id().map(SessionId::as_str),
            Some("session_process_affinity")
        );
    }

    #[test]
    fn sync_run_session_affinity_uses_durable_session_only_when_context_has_no_session_id() {
        let agent = AgentBuilder::new(Arc::new(FunctionModel::new(
            |_messages: Vec<ModelMessage>,
             _settings: Option<ModelSettings>,
             _info: FunctionModelInfo| { Ok(ModelResponse::text("ok")) },
        )))
        .build();
        let mut session = AgentSession::new(agent);
        session.set_session_id(SessionId::from_string("session_restored_affinity"));
        let run = RunRecord::new(
            SessionId::from_string("session_durable"),
            RunId::from_string("run_affinity"),
            ConversationId::from_string("conversation_affinity"),
        );

        sync_run_session_affinity(&mut session, &run);

        assert_eq!(
            session.context().session_id().map(SessionId::as_str),
            Some("session_restored_affinity")
        );
    }

    #[test]
    fn agent_session_passes_cli_run_ids_as_model_request_metadata() {
        let captured_settings = Arc::new(Mutex::new(Vec::<Option<ModelSettings>>::new()));
        let captured_metadata = Arc::new(Mutex::new(Vec::<
            serde_json::Map<String, serde_json::Value>,
        >::new()));
        let model_settings = Arc::clone(&captured_settings);
        let model_metadata = Arc::clone(&captured_metadata);
        let model = FunctionModel::new(
            move |_messages: Vec<ModelMessage>,
                  settings: Option<ModelSettings>,
                  info: FunctionModelInfo| {
                model_settings.lock().unwrap().push(settings);
                model_metadata
                    .lock()
                    .unwrap()
                    .push(info.context.llm_trace_metadata);
                Ok(ModelResponse::text("ok"))
            },
        );
        let agent = AgentBuilder::new(Arc::new(model)).build();
        let mut session = AgentSession::new(agent);
        let run = RunRecord::new(
            SessionId::from_string("session_runtime_header"),
            RunId::from_string("run_runtime_header"),
            ConversationId::from_string("conversation_runtime_header"),
        );
        sync_run_request_metadata(&mut session, &run);

        tokio::runtime::Runtime::new()
            .expect("runtime should start")
            .block_on(session.run_stream("hello"))
            .expect("run should succeed");

        let (captured_len, captured_has_empty_headers) = {
            let captured = captured_settings.lock().unwrap();
            (
                captured.len(),
                captured[0]
                    .as_ref()
                    .is_none_or(|settings| settings.extra_headers.is_empty()),
            )
        };
        assert_eq!(captured_len, 1);
        assert!(captured_has_empty_headers);
        let metadata = {
            let metadata = captured_metadata.lock().unwrap();
            metadata[0].clone()
        };
        assert_eq!(
            metadata["starweaver.durable_session_id"],
            "session_runtime_header"
        );
        assert_eq!(metadata["starweaver.durable_run_id"], "run_runtime_header");
        assert_eq!(metadata["cli.session_id"], "session_runtime_header");
        assert_eq!(metadata["cli.run_id"], "run_runtime_header");
    }

    #[test]
    fn prompt_content_adapter_replaces_initial_user_prompt_with_multimodal_parts() {
        let attachment = PromptAttachment::image(1, b"image-bytes".to_vec(), "image/png");
        let placeholder = attachment.placeholder.clone();
        let adapter = CliPromptContentAdapter {
            content_parts: crate::prompt_input::PromptInput {
                text: format!("inspect this {placeholder} now"),
                attachments: vec![attachment],
                extra_text_parts: vec!["Extra context for this prompt.".to_string()],
                guidance_text_parts: Vec::new(),
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
                ContentPart::Text {
                    text: "Extra context for this prompt.".to_string(),
                },
            ]
        );
        assert_eq!(metadata["starweaver.cli.attachments"], 1);
    }

    #[test]
    fn prompt_content_adapter_appends_extra_text_parts_to_initial_request() {
        let adapter = CliPromptContentAdapter {
            content_parts: crate::prompt_input::PromptInput {
                text: "implement feature".to_string(),
                attachments: Vec::new(),
                extra_text_parts: vec![
                    "Extra context one.".to_string(),
                    "Extra context two.".to_string(),
                ],
                guidance_text_parts: Vec::new(),
            }
            .into_content_parts(),
        };
        let mut state = AgentRunState::new(
            RunId::from_string("run_guidance"),
            ConversationId::from_string("conversation_guidance"),
        );
        let mut request = ModelRequest::user_text("implement feature");
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
        assert_eq!(metadata["starweaver.cli.attachments"], 0);
        assert_eq!(content.len(), 3);
        assert!(matches!(
            &content[0],
            ContentPart::Text { text } if text == "implement feature"
        ));
        assert!(matches!(
            &content[1],
            ContentPart::Text { text } if text == "Extra context one."
        ));
        assert!(matches!(
            &content[2],
            ContentPart::Text { text } if text == "Extra context two."
        ));
    }

    #[test]
    fn cli_guidance_adapter_injects_guidance_as_cacheable_system_prompts() {
        let adapter = CliGuidanceAdapter {
            guidance_text_parts: vec![
                "<project-guidance name=AGENTS.md>\nUse cargo test.\n</project-guidance>"
                    .to_string(),
                "<user-rules location=/home/user/.starweaver/RULES.md>\nPrefer Chinese replies.\n</user-rules>"
                    .to_string(),
            ],
        };
        let mut state = AgentRunState::new(
            RunId::from_string("run_guidance"),
            ConversationId::from_string("conversation_guidance"),
        );
        let mut context = AgentContext::default();
        let mut dynamic_metadata = serde_json::Map::new();
        dynamic_metadata.insert(
            INSTRUCTION_DYNAMIC_METADATA.to_string(),
            serde_json::json!(true),
        );
        let request = ModelRequest {
            parts: vec![
                ModelRequestPart::SystemPrompt {
                    text: "stable system".to_string(),
                    metadata: serde_json::Map::new(),
                },
                ModelRequestPart::Instruction {
                    text: "<environment-context>fresh</environment-context>".to_string(),
                    metadata: dynamic_metadata,
                },
                ModelRequestPart::UserPrompt {
                    content: vec![ContentPart::Text {
                        text: "implement feature".to_string(),
                    }],
                    name: None,
                    metadata: serde_json::Map::new(),
                },
            ],
            timestamp: None,
            instructions: None,
            run_id: None,
            conversation_id: None,
            metadata: serde_json::Map::new(),
        };

        let messages = tokio::runtime::Runtime::new()
            .expect("runtime should start")
            .block_on(adapter.prepare_model_messages_with_context(
                &mut state,
                &mut context,
                vec![ModelMessage::Request(request)],
            ))
            .expect("adapter should inject guidance");

        let ModelMessage::Request(request) = messages.last().expect("request") else {
            panic!("expected request");
        };
        assert!(matches!(
            &request.parts[0],
            ModelRequestPart::SystemPrompt { text, .. } if text == "stable system"
        ));
        assert!(matches!(
            &request.parts[1],
            ModelRequestPart::SystemPrompt { text, metadata }
                if metadata.get(INSTRUCTION_ORIGIN_METADATA)
                    == Some(&serde_json::json!(CLI_GUIDANCE_ORIGIN))
                    && text.contains("<project-guidance name=AGENTS.md>")
        ));
        assert!(matches!(
            &request.parts[2],
            ModelRequestPart::SystemPrompt { text, metadata }
                if metadata.get(INSTRUCTION_ORIGIN_METADATA)
                    == Some(&serde_json::json!(CLI_GUIDANCE_ORIGIN))
                    && text.contains("<user-rules location=/home/user/.starweaver/RULES.md>")
        ));
        assert!(matches!(
            &request.parts[3],
            ModelRequestPart::Instruction { text, metadata }
                if text.contains("<environment-context>fresh</environment-context>")
                    && metadata.get(INSTRUCTION_DYNAMIC_METADATA) == Some(&serde_json::json!(true))
        ));
        assert!(matches!(
            &request.parts[4],
            ModelRequestPart::UserPrompt { content, .. }
                if matches!(&content[0], ContentPart::Text { text } if text == "implement feature")
        ));
    }

    #[test]
    fn cli_guidance_replaces_stale_guidance_by_source_key() {
        let old_project =
            "<project-guidance name=AGENTS.md>\nOld project rules.\n</project-guidance>"
                .to_string();
        let new_project =
            "<project-guidance name=AGENTS.md>\nNew project rules.\n</project-guidance>"
                .to_string();
        let old_rules =
            "<user-rules location=/home/user/RULES.md>\nOld user rules.\n</user-rules>".to_string();
        let adapter = CliGuidanceAdapter {
            guidance_text_parts: vec![new_project.clone()],
        };
        let mut metadata = serde_json::Map::new();
        metadata.insert(
            INSTRUCTION_ORIGIN_METADATA.to_string(),
            serde_json::json!(CLI_GUIDANCE_ORIGIN),
        );
        let mut keyed_metadata = metadata.clone();
        keyed_metadata.insert(
            CLI_GUIDANCE_KEY_METADATA.to_string(),
            serde_json::json!("user-rules location=/home/user/RULES.md"),
        );
        let mut latest_metadata = serde_json::Map::new();
        latest_metadata.insert(
            INSTRUCTION_ORIGIN_METADATA.to_string(),
            serde_json::json!(CLI_GUIDANCE_ORIGIN),
        );
        latest_metadata.insert(
            CLI_GUIDANCE_KEY_METADATA.to_string(),
            serde_json::json!("project-guidance name=AGENTS.md"),
        );
        let messages = vec![
            ModelMessage::Request(ModelRequest {
                parts: vec![
                    ModelRequestPart::SystemPrompt {
                        text: old_project.clone(),
                        metadata,
                    },
                    ModelRequestPart::SystemPrompt {
                        text: old_rules.clone(),
                        metadata: keyed_metadata,
                    },
                ],
                timestamp: None,
                instructions: None,
                run_id: None,
                conversation_id: None,
                metadata: serde_json::Map::new(),
            }),
            ModelMessage::Response(ModelResponse::text("ok")),
            ModelMessage::Request(ModelRequest {
                parts: vec![ModelRequestPart::SystemPrompt {
                    text: old_project,
                    metadata: latest_metadata,
                }],
                timestamp: None,
                instructions: None,
                run_id: None,
                conversation_id: None,
                metadata: serde_json::Map::new(),
            }),
        ];
        let mut state = AgentRunState::new(
            RunId::from_string("run_guidance_replace"),
            ConversationId::from_string("conversation_guidance_replace"),
        );
        let mut context = AgentContext::default();

        let messages = tokio::runtime::Runtime::new()
            .expect("runtime should start")
            .block_on(adapter.prepare_model_messages_with_context(
                &mut state,
                &mut context,
                messages,
            ))
            .expect("adapter should update guidance");

        assert_eq!(count_guidance_in_messages(&messages, &new_project), 1);
        assert_eq!(count_guidance_in_messages(&messages, &old_rules), 0);
        let serialized = serde_json::to_string(&messages).expect("messages should serialize");
        assert!(serialized.contains("New project rules."));
        assert!(!serialized.contains("Old project rules."));
        assert!(!serialized.contains("Old user rules."));
    }

    #[test]
    fn cli_guidance_keeps_openai_responses_instruction_order_stable_for_tool_loops() {
        let guidance =
            "<project-guidance name=AGENTS.md>\nUse cargo test.\n</project-guidance>".to_string();
        let adapter = CliGuidanceAdapter {
            guidance_text_parts: vec![guidance.clone()],
        };
        let mut state = AgentRunState::new(
            RunId::from_string("run_guidance_tool_loop"),
            ConversationId::from_string("conversation_guidance_tool_loop"),
        );
        let mut context = AgentContext::default();
        let first_messages = prepare_guidance_messages(
            &adapter,
            &mut state,
            &mut context,
            first_guidance_messages(),
        );
        let first_wire = openai_responses_wire(&first_messages);
        assert_eq!(
            first_wire["instructions"],
            format!("stable agent\n\n{guidance}")
        );

        let mut second_messages = first_messages;
        second_messages.push(ModelMessage::Response(ModelResponse::text("assistant")));
        second_messages.push(ModelMessage::Request(ModelRequest {
            parts: vec![
                ModelRequestPart::ToolReturn(starweaver_model::ToolReturnPart::new(
                    "call_1",
                    "lookup",
                    serde_json::json!({"ok": true}),
                )),
                stable_agent_instruction(),
            ],
            timestamp: None,
            instructions: None,
            run_id: None,
            conversation_id: None,
            metadata: serde_json::Map::new(),
        }));
        let second_messages =
            prepare_guidance_messages(&adapter, &mut state, &mut context, second_messages);
        assert_eq!(count_guidance_in_messages(&second_messages, &guidance), 1);
        assert_first_request_has_no_guidance(&second_messages);
        assert_latest_request_has_stable_agent_then_guidance(&second_messages, &guidance);
        let second_wire = openai_responses_wire(&second_messages);
        assert_eq!(second_wire["instructions"], first_wire["instructions"]);
    }

    #[test]
    fn cli_guidance_is_cacheable_and_deduped_in_session_history() {
        let captured_messages = Arc::new(Mutex::new(Vec::<Vec<ModelMessage>>::new()));
        let model_captured = Arc::clone(&captured_messages);
        let model = FunctionModel::new(
            move |messages: Vec<ModelMessage>,
                  _settings: Option<ModelSettings>,
                  _info: FunctionModelInfo| {
                model_captured.lock().unwrap().push(messages);
                Ok(ModelResponse::text("ok"))
            },
        );
        let guidance =
            "<project-guidance name=AGENTS.md>\nUse cargo test.\n</project-guidance>".to_string();
        let agent = AgentBuilder::new(Arc::new(model))
            .capability(Arc::new(CliGuidanceAdapter {
                guidance_text_parts: vec![guidance.clone(), guidance.clone()],
            }))
            .build();
        let mut session = AgentSession::new(agent);
        let runtime = tokio::runtime::Runtime::new().expect("runtime should start");

        let first_result = runtime
            .block_on(session.run_stream("implement feature"))
            .expect("first run should succeed");
        assert_eq!(first_result.result.output, "ok");
        let second_result = runtime
            .block_on(session.run_stream("continue feature"))
            .expect("second run should succeed");
        assert_eq!(second_result.result.output, "ok");

        {
            let captured = captured_messages.lock().unwrap();
            assert_eq!(captured.len(), 2);
            assert_eq!(count_guidance_in_messages(&captured[0], &guidance), 1);
            assert_eq!(count_guidance_in_messages(&captured[1], &guidance), 1);

            let first_wire =
                OpenAiResponsesAdapter::build_request("gpt-5.5", &captured[0], None, &[], &[])
                    .expect("first wire request should build");
            let second_wire =
                OpenAiResponsesAdapter::build_request("gpt-5.5", &captured[1], None, &[], &[])
                    .expect("second wire request should build");
            assert_eq!(first_wire["instructions"], guidance);
            assert_eq!(second_wire["instructions"], first_wire["instructions"]);
            let first_input = stable_wire_input_items(&first_wire);
            let second_input = stable_wire_input_items(&second_wire);
            assert!(second_input.len() > first_input.len());
            assert_eq!(first_input.as_slice(), &second_input[..first_input.len()]);
            drop(captured);
        }
        let persisted = serde_json::to_string(&session.export_full_state().message_history)
            .expect("message history should serialize");
        assert!(persisted.contains("project-guidance"));
        assert!(persisted.contains("Use cargo test."));
    }

    fn stable_agent_instruction() -> ModelRequestPart {
        ModelRequestPart::Instruction {
            text: "stable agent".to_string(),
            metadata: serde_json::Map::from_iter([
                (
                    INSTRUCTION_ORIGIN_METADATA.to_string(),
                    serde_json::json!("agent_instruction"),
                ),
                (
                    INSTRUCTION_DYNAMIC_METADATA.to_string(),
                    serde_json::json!(false),
                ),
            ]),
        }
    }

    fn first_guidance_messages() -> Vec<ModelMessage> {
        vec![ModelMessage::Request(ModelRequest {
            parts: vec![
                stable_agent_instruction(),
                ModelRequestPart::UserPrompt {
                    content: vec![ContentPart::Text {
                        text: "first".to_string(),
                    }],
                    name: None,
                    metadata: serde_json::Map::new(),
                },
            ],
            timestamp: None,
            instructions: None,
            run_id: None,
            conversation_id: None,
            metadata: serde_json::Map::new(),
        })]
    }

    fn prepare_guidance_messages(
        adapter: &CliGuidanceAdapter,
        state: &mut AgentRunState,
        context: &mut AgentContext,
        messages: Vec<ModelMessage>,
    ) -> Vec<ModelMessage> {
        tokio::runtime::Runtime::new()
            .expect("runtime should start")
            .block_on(adapter.prepare_model_messages_with_context(state, context, messages))
            .expect("messages should prepare")
    }

    fn openai_responses_wire(messages: &[ModelMessage]) -> serde_json::Value {
        OpenAiResponsesAdapter::build_request("gpt-5.5", messages, None, &[], &[])
            .expect("wire request should build")
    }

    fn assert_first_request_has_no_guidance(messages: &[ModelMessage]) {
        let ModelMessage::Request(first_request) = &messages[0] else {
            panic!("expected first request");
        };
        assert!(
            !first_request
                .parts
                .iter()
                .any(|part| cli_guidance_key(part).is_some())
        );
    }

    fn assert_latest_request_has_stable_agent_then_guidance(
        messages: &[ModelMessage],
        guidance: &str,
    ) {
        let latest_request = messages
            .iter()
            .rev()
            .find_map(|message| match message {
                ModelMessage::Request(request) => Some(request),
                ModelMessage::Response(_) => None,
            })
            .expect("latest request");
        assert!(matches!(
            &latest_request.parts[1],
            ModelRequestPart::Instruction { text, .. } if text == "stable agent"
        ));
        assert!(matches!(
            &latest_request.parts[2],
            ModelRequestPart::SystemPrompt { text, metadata }
                if text == guidance
                    && metadata.get(INSTRUCTION_ORIGIN_METADATA)
                        == Some(&serde_json::json!(CLI_GUIDANCE_ORIGIN))
        ));
    }

    fn stable_wire_input_items(wire: &serde_json::Value) -> Vec<serde_json::Value> {
        wire["input"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| !is_runtime_context_wire_item(item))
            .cloned()
            .collect()
    }

    fn is_runtime_context_wire_item(item: &serde_json::Value) -> bool {
        item.get("role").and_then(serde_json::Value::as_str) == Some("user")
            && (item
                .get(CONTEXT_ORIGIN_METADATA)
                .and_then(serde_json::Value::as_str)
                == Some(CONTEXT_ORIGIN_RUNTIME_CONTEXT)
                || item["content"].as_array().is_some_and(|content| {
                    content.iter().any(|part| {
                        part.get("text")
                            .and_then(serde_json::Value::as_str)
                            .is_some_and(|text| text.starts_with("<runtime-context>"))
                    })
                }))
    }

    fn count_guidance_in_messages(messages: &[ModelMessage], guidance: &str) -> usize {
        messages
            .iter()
            .flat_map(|message| match message {
                ModelMessage::Request(request) => request.parts.iter().collect::<Vec<_>>(),
                ModelMessage::Response(_) => Vec::new(),
            })
            .filter(|part| match part {
                ModelRequestPart::SystemPrompt { text, metadata }
                | ModelRequestPart::Instruction { text, metadata } => {
                    metadata.get(INSTRUCTION_ORIGIN_METADATA)
                        == Some(&serde_json::json!(CLI_GUIDANCE_ORIGIN))
                        && text == guidance
                }
                ModelRequestPart::UserPrompt { content, metadata, .. } => {
                    metadata.get(INSTRUCTION_ORIGIN_METADATA)
                        == Some(&serde_json::json!(CLI_GUIDANCE_ORIGIN))
                        && matches!(content.first(), Some(ContentPart::Text { text }) if text == guidance)
                }
                ModelRequestPart::ToolReturn(_) | ModelRequestPart::RetryPrompt { .. } => false,
            })
            .count()
    }

    #[test]
    fn prompt_content_adapter_only_updates_first_model_step() {
        let adapter = CliPromptContentAdapter {
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
    fn steering_adapter_drains_messages_synchronously_at_the_guard_boundary() {
        let channel = CliSteeringChannel::new();
        let mut context = AgentContext::default();
        context.subscribe_messages();

        channel
            .submit(CliSteeringMessage {
                id: "steer_test".to_string(),
                text: "tighten scroll".to_string(),
            })
            .expect("steering channel should remain open");

        assert!(channel.drain_into(&mut context, false));
        let messages = context.messages.peek(context.agent_id.as_str());
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content["id"], "steer_test");
        assert_eq!(messages[0].content["text"], "tighten scroll");
    }

    #[test]
    fn empty_final_drain_seals_steering_admission_without_silent_acceptance() {
        let channel = CliSteeringChannel::new();
        let mut context = AgentContext::default();
        context.subscribe_messages();

        assert!(!channel.drain_into(&mut context, true));
        let message = CliSteeringMessage {
            id: "steer_after_seal".to_string(),
            text: "preserve this correction".to_string(),
        };
        assert_eq!(channel.submit(message.clone()), Err(message));
    }

    #[test]
    fn nonempty_final_drain_keeps_steering_admission_open_for_guard_turn() {
        let channel = CliSteeringChannel::new();
        let mut context = AgentContext::default();
        context.subscribe_messages();
        channel
            .submit(CliSteeringMessage {
                id: "steer_first".to_string(),
                text: "first correction".to_string(),
            })
            .unwrap();

        assert!(channel.drain_into(&mut context, true));
        assert!(
            channel
                .submit(CliSteeringMessage {
                    id: "steer_second".to_string(),
                    text: "second correction".to_string(),
                })
                .is_ok()
        );
    }

    #[test]
    fn steering_sent_before_final_validation_is_reinjected_before_completion() {
        let channel = CliSteeringChannel::new();
        let model_channel = channel.clone();
        let requests = Arc::new(Mutex::new(Vec::<Vec<ModelMessage>>::new()));
        let captured_requests = Arc::clone(&requests);
        let request_count = Arc::new(Mutex::new(0_usize));
        let model_request_count = Arc::clone(&request_count);
        let model = FunctionModel::new(
            move |messages: Vec<ModelMessage>,
                  _settings: Option<ModelSettings>,
                  _info: FunctionModelInfo| {
                captured_requests.lock().unwrap().push(messages);
                let mut count = model_request_count.lock().unwrap();
                *count += 1;
                if *count == 1 {
                    model_channel
                        .submit(CliSteeringMessage {
                            id: "steer_late".to_string(),
                            text: "include the late correction".to_string(),
                        })
                        .expect("steering channel should remain open");
                    Ok(ModelResponse::text("first answer"))
                } else {
                    Ok(ModelResponse::text("corrected answer"))
                }
            },
        );
        let agent = AgentBuilder::new(Arc::new(model))
            .build()
            .with_capability(Arc::new(CliSteeringAdapter { channel }));
        let mut session = AgentSession::new(agent);

        let stream = tokio::runtime::Runtime::new()
            .expect("runtime should start")
            .block_on(session.run_stream("hello"))
            .expect("steering guard should continue the run");

        assert_eq!(stream.result.output, "corrected answer");
        assert_eq!(*request_count.lock().unwrap(), 2);
        let second_request = serde_json::to_string(&requests.lock().unwrap()[1]).unwrap();
        assert!(second_request.contains("include the late correction"));
        assert!(
            stream
                .events
                .iter()
                .any(|record| matches!(&record.event, AgentStreamEvent::SteeringGuard { .. }))
        );
        assert!(stream.events.iter().any(|record| matches!(
            &record.event,
            AgentStreamEvent::Custom { event }
                if event.kind == "steering_received" && event.payload["id"] == "steer_late"
        )));
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
        assert!(
            !serde_json::to_string(&partial)
                .unwrap()
                .contains("unfinished reasoning")
        );
    }

    #[test]
    fn run_session_stream_returns_cancelled_when_cancel_receiver_fires() {
        let (cancel_sender, cancel_receiver) = mpsc::channel::<()>();
        let (result_sender, result_receiver) = mpsc::channel::<Result<(), String>>();
        let worker = thread::spawn(move || {
            let runtime = tokio::runtime::Runtime::new().expect("runtime should start");
            let agent = AgentBuilder::new(Arc::new(NeverEndingStreamModel::new())).build();
            let mut session = AgentSession::new(agent);
            let result = match run_session_stream(
                &runtime,
                &mut session,
                "hello".to_string(),
                vec![cancel_receiver],
                Some(CancellationToken::new()),
            ) {
                Ok(SessionRunOutcome::Cancelled) => Ok(()),
                Ok(SessionRunOutcome::Completed(_)) => {
                    Err("run completed instead of cancelling".to_string())
                }
                Ok(SessionRunOutcome::Failed(error)) => {
                    Err(format!("run failed instead of cancelling: {error}"))
                }
                Err(error) => Err(format!("runner returned error: {error}")),
            };
            result_sender
                .send(result)
                .expect("test result receiver should remain open");
        });

        thread::sleep(Duration::from_millis(50));
        cancel_sender
            .send(())
            .expect("cancel receiver should remain open");
        result_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("cancelled run should return quickly")
            .expect("run should be cancelled");
        worker.join().expect("runner worker should not panic");
    }

    struct NeverEndingStreamModel {
        profile: ModelProfile,
        senders:
            Mutex<Vec<tokio::sync::mpsc::Sender<Result<ModelResponseStreamEvent, ModelError>>>>,
    }

    impl NeverEndingStreamModel {
        fn new() -> Self {
            Self {
                profile: ModelProfile::for_protocol(ProtocolFamily::OpenAiChatCompletions),
                senders: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl ModelAdapter for NeverEndingStreamModel {
        fn model_name(&self) -> &'static str {
            "never-ending"
        }

        fn provider_name(&self) -> Option<&str> {
            Some("test")
        }

        fn profile(&self) -> &ModelProfile {
            &self.profile
        }

        fn default_settings(&self) -> Option<&ModelSettings> {
            None
        }

        async fn request(
            &self,
            _messages: Vec<ModelMessage>,
            _settings: Option<ModelSettings>,
            _params: ModelRequestParameters,
            _context: ModelRequestContext,
        ) -> Result<ModelResponse, ModelError> {
            Ok(ModelResponse::text("unexpected"))
        }

        async fn request_stream_incremental(
            &self,
            _messages: Vec<ModelMessage>,
            _settings: Option<ModelSettings>,
            _params: ModelRequestParameters,
            context: ModelRequestContext,
        ) -> Result<ModelResponseEventStream, ModelError> {
            let (sender, receiver) = tokio::sync::mpsc::channel(1);
            self.senders
                .lock()
                .expect("model sender lock should be available")
                .push(sender);
            Ok(ModelResponseEventStream::new_with_cancellation(
                receiver,
                context.cancellation_token(),
            ))
        }
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
            &CliRunPolicy {
                hitl: HitlPolicy::Deny,
                goal: None,
            },
            &runtime,
        );

        assert_eq!(projection.status, RunStatus::Cancelled);
        assert!(
            projection
                .messages
                .iter()
                .any(|message| message.kind == DisplayMessageKind::RunStarted)
        );
        assert!(
            projection
                .messages
                .iter()
                .any(|message| message.kind == DisplayMessageKind::RunCancelled)
        );
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
