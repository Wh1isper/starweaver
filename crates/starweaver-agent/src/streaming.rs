//! Live SDK streaming helpers.

use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use serde_json::Value;
use starweaver_context::{AgentContext, AgentEvent, BusMessage, ResumableState};
use starweaver_core::{CancellationToken, Metadata};
use starweaver_runtime::{
    AgentCapability, AgentError, AgentInput, AgentResult, AgentRunState, AgentStreamEvent,
    AgentStreamRecord, AgentStreamResult, CapabilityError, CapabilityResult, CapabilitySpec,
};
use thiserror::Error;
use tokio::{
    sync::mpsc::{self, error::TrySendError},
    task::{JoinError, JoinHandle},
};

use crate::AgentSession;
use crate::session::{RunContextRestore, restore_context_overrides};

const DEFAULT_STREAM_BUFFER: usize = 256;
const INTERRUPT_JOIN_TIMEOUT: Duration = Duration::from_secs(5);
const CONTROL_DRAIN_CAPABILITY_ID: &str = "starweaver.agent.active_control.drain";
const TRACE_METADATA_STATE_KEY: &str = "starweaver.trace_metadata";

/// Backpressure behavior for live SDK streams.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AgentStreamDropPolicy {
    /// Drop the newest event when the receiver falls behind.
    #[default]
    DropNewest,
    /// Wait for receiver capacity instead of dropping events.
    Backpressure,
}

/// Options controlling live SDK stream delivery.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AgentStreamOptions {
    /// Number of stream records buffered between producer and receiver.
    pub buffer_size: usize,
    /// Delivery policy when the buffer is full.
    pub drop_policy: AgentStreamDropPolicy,
}

impl AgentStreamOptions {
    /// Create default stream options.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            buffer_size: DEFAULT_STREAM_BUFFER,
            drop_policy: AgentStreamDropPolicy::DropNewest,
        }
    }

    /// Set the stream buffer size. Values below one are clamped at construction time.
    #[must_use]
    pub const fn buffer_size(mut self, buffer_size: usize) -> Self {
        self.buffer_size = buffer_size;
        self
    }

    /// Set the drop/backpressure policy.
    #[must_use]
    pub const fn drop_policy(mut self, drop_policy: AgentStreamDropPolicy) -> Self {
        self.drop_policy = drop_policy;
        self
    }

    fn normalized(self) -> Self {
        Self {
            buffer_size: self.buffer_size.max(1),
            drop_policy: self.drop_policy,
        }
    }
}

impl Default for AgentStreamOptions {
    fn default() -> Self {
        Self::new()
    }
}

/// Completed live stream output plus the final session context.
#[derive(Debug)]
pub struct AgentLiveStreamResult {
    /// Final agent result.
    pub result: AgentResult,
    /// Final context after the stream run.
    pub context: AgentContext,
    /// Stream records captured by the runtime during the run.
    pub events: Vec<AgentStreamRecord>,
}

impl AgentLiveStreamResult {
    /// Convert into the collected stream result.
    #[must_use]
    pub fn into_stream_result(self) -> AgentStreamResult {
        AgentStreamResult {
            result: self.result,
            events: self.events,
        }
    }
}

/// Non-raising live stream completion state.
#[derive(Debug)]
pub struct AgentStreamCompletion {
    /// Final stream result when the run completed successfully.
    pub result: Option<AgentLiveStreamResult>,
    /// Completion error when the run failed, was interrupted, or the task failed.
    pub error: Option<AgentStreamError>,
    /// Latest recoverable session state observed by the stream.
    pub state: ResumableState,
    /// Stream records observed before completion or failure.
    pub events: Vec<AgentStreamRecord>,
}

impl AgentStreamCompletion {
    /// Return whether the stream completed successfully.
    #[must_use]
    pub const fn is_ok(&self) -> bool {
        self.error.is_none()
    }

    /// Return whether the stream completed with an error.
    #[must_use]
    pub const fn is_err(&self) -> bool {
        self.error.is_some()
    }
}

/// High-level live stream run status for polling UIs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentStreamRunStatus {
    /// Producer task is still running.
    Running,
    /// Cooperative cancellation has been requested and the producer has not finished.
    Cancelling,
    /// Producer task has finished.
    Finished,
}

/// Cloneable current error snapshot for a live stream.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AgentStreamCurrentError {
    /// The caller deliberately interrupted the live stream.
    Interrupted,
    /// The underlying agent run failed.
    Agent(String),
    /// The runtime task failed before returning an agent result.
    Join(String),
}

/// Accepted live-run control input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentControlReceipt {
    /// Accepted control id.
    pub id: String,
    /// Accepted control kind.
    pub kind: AgentControlKind,
    /// Whether the input was queued for active-run delivery.
    pub queued: bool,
    /// Active run id when known at enqueue time.
    pub run_id: Option<String>,
    /// Active session id when known at enqueue time.
    pub session_id: Option<String>,
}

/// Kind of accepted live-run control input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentControlKind {
    /// Generic message-bus write.
    Message,
    /// User steering message.
    Steering,
    /// Run interruption.
    Interrupt,
}

impl AgentControlKind {
    /// Return the stable Python/API name for this control kind.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Message => "message",
            Self::Steering => "steering",
            Self::Interrupt => "interrupt",
        }
    }
}

/// Cloneable control handle for a live SDK run.
#[derive(Clone)]
pub struct AgentControlHandle {
    interrupted: Arc<AtomicBool>,
    interrupt_reason: Arc<Mutex<Option<String>>>,
    cancellation_token: CancellationToken,
    latest_context: Arc<tokio::sync::Mutex<AgentContext>>,
    pending_messages: Arc<tokio::sync::Mutex<VecDeque<BusMessage>>>,
    finished: Arc<AtomicBool>,
}

impl AgentControlHandle {
    /// Request cooperative cancellation of the active run.
    #[must_use]
    pub fn interrupt(&self, reason: Option<String>) -> AgentControlReceipt {
        let reason = reason.unwrap_or_else(|| "agent stream interruption requested".to_string());
        match self.interrupt_reason.lock() {
            Ok(mut stored) => *stored = Some(reason),
            Err(error) => *error.into_inner() = Some(reason),
        }
        self.interrupted.store(true, Ordering::SeqCst);
        self.cancellation_token.cancel();
        AgentControlReceipt {
            id: "interrupt".to_string(),
            kind: AgentControlKind::Interrupt,
            queued: false,
            run_id: None,
            session_id: None,
        }
    }

    /// Queue a message for injection into the active runtime context.
    ///
    /// # Errors
    ///
    /// Returns [`AgentControlError::TerminalRun`] when the run has already finished.
    pub async fn send_message(
        &self,
        message: BusMessage,
    ) -> Result<AgentControlReceipt, AgentControlError> {
        if self.finished.load(Ordering::SeqCst) || self.interrupted.load(Ordering::SeqCst) {
            return Err(AgentControlError::TerminalRun);
        }
        let kind = control_kind_for_message(&message);
        let (run_id, session_id) = {
            let context = self.latest_context.lock().await;
            (
                context
                    .run_id
                    .as_ref()
                    .map(|run_id| run_id.as_str().to_string()),
                context
                    .session_id
                    .as_ref()
                    .map(|session_id| session_id.as_str().to_string()),
            )
        };
        let receipt = AgentControlReceipt {
            id: message.id.clone(),
            kind,
            queued: true,
            run_id,
            session_id,
        };
        {
            let mut pending = self.pending_messages.lock().await;
            if self.finished.load(Ordering::SeqCst) || self.interrupted.load(Ordering::SeqCst) {
                return Err(AgentControlError::TerminalRun);
            }
            pending.push_back(message);
        }
        Ok(receipt)
    }

    /// Queue a user steering message for injection into the active runtime context.
    ///
    /// # Errors
    ///
    /// Returns [`AgentControlError::TerminalRun`] when the run has already finished.
    pub async fn steer(
        &self,
        id: impl Into<String>,
        text: impl Into<String>,
    ) -> Result<AgentControlReceipt, AgentControlError> {
        let id = id.into();
        let text = text.into();
        let mut message = BusMessage::text(text, "user").with_id(id.clone());
        message.metadata.insert(
            "starweaver.topic".to_string(),
            serde_json::json!("steering"),
        );
        let mut receipt = self.send_message(message).await?;
        receipt.kind = AgentControlKind::Steering;
        Ok(receipt)
    }

    /// Export the latest observed recoverable run state.
    pub async fn recoverable_state(&self) -> ResumableState {
        self.latest_context.lock().await.export_full_state()
    }

    fn drain_capability(&self) -> Arc<dyn AgentCapability> {
        Arc::new(AgentControlDrainCapability {
            pending_messages: self.pending_messages.clone(),
            finished: self.finished.clone(),
        })
    }

    fn mark_finished(&self) {
        self.finished.store(true, Ordering::SeqCst);
    }

    fn interrupt_reason(&self) -> String {
        interrupt_reason(&self.interrupt_reason)
    }
}

fn new_control_handle(
    interrupted: Arc<AtomicBool>,
    interrupt_reason: Arc<Mutex<Option<String>>>,
    cancellation_token: CancellationToken,
    latest_context: Arc<tokio::sync::Mutex<AgentContext>>,
) -> AgentControlHandle {
    AgentControlHandle {
        interrupted,
        interrupt_reason,
        cancellation_token,
        latest_context,
        pending_messages: Arc::new(tokio::sync::Mutex::new(VecDeque::new())),
        finished: Arc::new(AtomicBool::new(false)),
    }
}

fn control_kind_for_message(message: &BusMessage) -> AgentControlKind {
    if message
        .metadata
        .get("starweaver.topic")
        .and_then(serde_json::Value::as_str)
        == Some("steering")
    {
        AgentControlKind::Steering
    } else {
        AgentControlKind::Message
    }
}

/// Errors raised by live-run control APIs.
#[derive(Debug, Error)]
pub enum AgentControlError {
    /// The control input targeted a run that has already terminated.
    #[error("agent run has already completed")]
    TerminalRun,
}

struct AgentControlDrainCapability {
    pending_messages: Arc<tokio::sync::Mutex<VecDeque<BusMessage>>>,
    finished: Arc<AtomicBool>,
}

#[async_trait]
impl AgentCapability for AgentControlDrainCapability {
    fn spec(&self) -> CapabilitySpec {
        CapabilitySpec::new(CONTROL_DRAIN_CAPABILITY_ID)
    }

    async fn prepare_run_input_with_context(
        &self,
        _state: &mut AgentRunState,
        context: &mut AgentContext,
        input: AgentInput,
    ) -> CapabilityResult<AgentInput> {
        self.drain(context).await;
        Ok(input)
    }

    async fn prepare_model_messages_with_context(
        &self,
        _state: &mut AgentRunState,
        context: &mut AgentContext,
        messages: Vec<starweaver_model::ModelMessage>,
    ) -> CapabilityResult<Vec<starweaver_model::ModelMessage>> {
        self.drain(context).await;
        Ok(messages)
    }

    async fn after_output_validation_with_context(
        &self,
        _state: &mut AgentRunState,
        context: &mut AgentContext,
        _output: &str,
    ) -> CapabilityResult<()> {
        self.drain_before_terminal_guard(context).await;
        Ok(())
    }
}

impl AgentControlDrainCapability {
    async fn drain(&self, context: &mut AgentContext) {
        let mut pending = self.pending_messages.lock().await;
        drain_pending_control_messages(&mut pending, context);
    }

    async fn drain_before_terminal_guard(&self, context: &mut AgentContext) {
        let mut pending = self.pending_messages.lock().await;
        drain_pending_control_messages(&mut pending, context);
        if !has_pending_runtime_steering(context) {
            self.finished.store(true, Ordering::SeqCst);
        }
    }
}

fn drain_pending_control_messages(pending: &mut VecDeque<BusMessage>, context: &mut AgentContext) {
    while let Some(message) = pending.pop_front() {
        let id = message.id.clone();
        let existed = context
            .messages
            .messages()
            .iter()
            .any(|existing| existing.id == id);
        let topic = message
            .metadata
            .get("starweaver.topic")
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned);
        let accepted = context.send_message(message);
        if existed {
            continue;
        }
        let event_kind = if topic.as_deref() == Some("steering") {
            "steering_submitted"
        } else {
            "message_submitted"
        };
        context.publish_event(AgentEvent::new(
            event_kind,
            serde_json::json!({
                "id": accepted.id,
                "topic": topic,
                "queued_id": id,
            }),
        ));
    }
}

fn has_pending_runtime_steering(context: &AgentContext) -> bool {
    context
        .messages
        .peek(context.agent_id.as_str())
        .iter()
        .any(is_runtime_steering_message)
}

fn is_runtime_steering_message(message: &BusMessage) -> bool {
    message
        .metadata
        .get("starweaver.topic")
        .and_then(serde_json::Value::as_str)
        == Some("steering")
}

/// Pollable live stream status snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentStreamStatus {
    /// High-level run status.
    pub run_status: AgentStreamRunStatus,
    /// Latest observed error when the producer has reached an error boundary.
    pub current_error: Option<AgentStreamCurrentError>,
    /// Whether cooperative cancellation has been requested.
    pub cancel_requested: bool,
    /// Number of live records dropped because the receiver lagged.
    pub dropped_events: usize,
    /// Whether the producer observed that the receiver was closed.
    pub receiver_closed: bool,
    /// Stream delivery options used by this handle.
    pub options: AgentStreamOptions,
}

/// Cloneable control handle for a live SDK stream run.
#[derive(Clone)]
pub struct AgentStreamController {
    interrupted: Arc<AtomicBool>,
    latest_context: Arc<tokio::sync::Mutex<AgentContext>>,
    control: AgentControlHandle,
}

impl AgentStreamController {
    /// Request cooperative cancellation of the running stream.
    pub fn interrupt(&self) {
        let _ = self.control.interrupt(None);
    }

    /// Return whether cooperative cancellation has been requested.
    #[must_use]
    pub fn cancel_requested(&self) -> bool {
        self.interrupted.load(Ordering::SeqCst)
    }

    /// Export the latest observed context state.
    pub async fn recoverable_state(&self) -> ResumableState {
        self.latest_context.lock().await.export_full_state()
    }

    /// Return the active-run control handle.
    #[must_use]
    pub fn control_handle(&self) -> AgentControlHandle {
        self.control.clone()
    }
}

/// Error returned by live stream handles.
#[derive(Debug, Error)]
pub enum AgentStreamError {
    /// Live stream construction was attempted without an active Tokio runtime.
    #[error("tokio runtime unavailable for live agent stream: {0}")]
    RuntimeUnavailable(String),
    /// The caller deliberately interrupted the live stream.
    #[error("agent stream interrupted: {reason}")]
    Interrupted {
        /// Human-readable cancellation reason.
        reason: String,
    },
    /// The runtime task failed before returning an agent result.
    #[error("agent stream task failed: {0}")]
    Join(String),
    /// The underlying agent run failed.
    #[error(transparent)]
    Agent(#[from] AgentError),
}

/// Handle for a live SDK stream run.
pub struct AgentStreamHandle {
    receiver: mpsc::Receiver<AgentStreamRecord>,
    join: JoinHandle<Result<AgentLiveStreamResult, AgentError>>,
    latest_context: Arc<tokio::sync::Mutex<AgentContext>>,
    interrupted: Arc<AtomicBool>,
    control: AgentControlHandle,
    dropped_events: Arc<AtomicUsize>,
    receiver_closed: Arc<AtomicBool>,
    current_error: Arc<Mutex<Option<AgentStreamCurrentError>>>,
    observed_events: Arc<Mutex<Vec<AgentStreamRecord>>>,
    options: AgentStreamOptions,
    temporary_run_context: Option<TemporaryRunContext>,
}

#[derive(Clone, Debug)]
struct TemporaryRunContext {
    restore: RunContextRestore,
    context_metadata: Metadata,
    trace_metadata: Metadata,
}

impl AgentStreamHandle {
    #[allow(clippy::too_many_arguments)]
    pub(crate) const fn new(
        receiver: mpsc::Receiver<AgentStreamRecord>,
        join: JoinHandle<Result<AgentLiveStreamResult, AgentError>>,
        latest_context: Arc<tokio::sync::Mutex<AgentContext>>,
        interrupted: Arc<AtomicBool>,
        control: AgentControlHandle,
        dropped_events: Arc<AtomicUsize>,
        receiver_closed: Arc<AtomicBool>,
        current_error: Arc<Mutex<Option<AgentStreamCurrentError>>>,
        observed_events: Arc<Mutex<Vec<AgentStreamRecord>>>,
        options: AgentStreamOptions,
    ) -> Self {
        Self {
            receiver,
            join,
            latest_context,
            interrupted,
            control,
            dropped_events,
            receiver_closed,
            current_error,
            observed_events,
            options,
            temporary_run_context: None,
        }
    }

    pub(crate) fn with_temporary_run_context(
        mut self,
        restore: RunContextRestore,
        context_metadata: Metadata,
        trace_metadata: Metadata,
    ) -> Self {
        self.temporary_run_context = Some(TemporaryRunContext {
            restore,
            context_metadata,
            trace_metadata,
        });
        self
    }

    pub(crate) fn with_optional_temporary_run_context(
        self,
        restore: Option<RunContextRestore>,
        context_metadata: Metadata,
        trace_metadata: Metadata,
    ) -> Self {
        match restore {
            Some(restore) => {
                self.with_temporary_run_context(restore, context_metadata, trace_metadata)
            }
            None => self,
        }
    }

    /// Receive the next live stream record.
    pub async fn recv(&mut self) -> Option<AgentStreamRecord> {
        self.receiver.recv().await
    }

    /// Try to receive a stream record without waiting.
    ///
    /// # Errors
    ///
    /// Returns `TryRecvError` when the stream is currently empty or closed.
    pub fn try_recv(&mut self) -> Result<AgentStreamRecord, mpsc::error::TryRecvError> {
        self.receiver.try_recv()
    }

    /// Close the receiver side while allowing the producer run to finish.
    pub fn close_receiver(&mut self) {
        self.receiver.close();
    }

    /// Return whether the producer task has finished.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.join.is_finished()
    }

    /// Return the stream delivery options used by this handle.
    #[must_use]
    pub const fn options(&self) -> AgentStreamOptions {
        self.options
    }

    /// Return the number of live records dropped because the receiver lagged.
    #[must_use]
    pub fn dropped_events(&self) -> usize {
        self.dropped_events.load(Ordering::SeqCst)
    }

    /// Return whether the producer observed that the receiver was closed.
    #[must_use]
    pub fn receiver_closed(&self) -> bool {
        self.receiver_closed.load(Ordering::SeqCst)
    }

    /// Return a pollable status snapshot for non-raising stream consumers.
    #[must_use]
    pub fn status(&self) -> AgentStreamStatus {
        let cancel_requested = self.cancel_requested();
        let run_status = if self.is_finished() {
            AgentStreamRunStatus::Finished
        } else if cancel_requested {
            AgentStreamRunStatus::Cancelling
        } else {
            AgentStreamRunStatus::Running
        };
        AgentStreamStatus {
            run_status,
            current_error: match self.current_error.lock() {
                Ok(error) => error.clone(),
                Err(error) => error.into_inner().clone(),
            },
            cancel_requested,
            dropped_events: self.dropped_events(),
            receiver_closed: self.receiver_closed(),
            options: self.options,
        }
    }

    /// Return a cloneable controller for this stream.
    #[must_use]
    pub fn controller(&self) -> AgentStreamController {
        AgentStreamController {
            interrupted: self.interrupted.clone(),
            latest_context: self.latest_context.clone(),
            control: self.control.clone(),
        }
    }

    /// Return the active-run control handle.
    #[must_use]
    pub fn control_handle(&self) -> AgentControlHandle {
        self.control.clone()
    }

    /// Request cooperative cancellation of the running stream.
    ///
    /// The producer observes this flag at stream/capability boundaries and exits with
    /// `AgentStreamError::Interrupted`. `join` still has a timeout-backed abort fallback
    /// for models or tools that never yield another boundary.
    pub fn interrupt(&self) {
        self.controller().interrupt();
    }

    /// Return whether cooperative cancellation has been requested.
    #[must_use]
    pub fn cancel_requested(&self) -> bool {
        self.interrupted.load(Ordering::SeqCst)
    }

    /// Export the latest observed context state.
    pub async fn recoverable_state(&self) -> ResumableState {
        let mut state = self.latest_context.lock().await.export_full_state();
        restore_temporary_state_context(&mut state, self.temporary_run_context.as_ref());
        state
    }

    /// Return the latest observed context clone.
    pub async fn latest_context(&self) -> AgentContext {
        let mut context = self.latest_context.lock().await.clone();
        restore_temporary_context(&mut context, self.temporary_run_context.as_ref());
        context
    }

    /// Wait for stream completion and return the final result.
    ///
    /// # Errors
    ///
    /// Returns an error when the caller interrupted the stream, the task panicked or
    /// was cancelled, or the underlying agent run failed.
    pub async fn join(self) -> Result<AgentLiveStreamResult, AgentStreamError> {
        let interrupted = self.interrupted.clone();
        let mut join = self.join;
        let control = self.control.clone();
        let temporary_run_context = self.temporary_run_context.clone();
        if interrupted.load(Ordering::SeqCst) {
            if let Ok(result) = tokio::time::timeout(INTERRUPT_JOIN_TIMEOUT, &mut join).await {
                control.mark_finished();
                let mut result = map_stream_join_result(result);
                if let Ok(result) = &mut result {
                    apply_temporary_run_context(result, temporary_run_context.as_ref());
                }
                return result;
            }
            join.abort();
            let _ = join.await;
            control.mark_finished();
            return Err(AgentStreamError::Interrupted {
                reason: control.interrupt_reason(),
            });
        }
        let mut result = map_stream_join_result(join.await);
        if let Ok(result) = &mut result {
            apply_temporary_run_context(result, temporary_run_context.as_ref());
        }
        control.mark_finished();
        result
    }

    /// Wait for stream completion and return result, error, and recoverable state
    /// without propagating failures.
    pub async fn complete(self) -> AgentStreamCompletion {
        let latest_context = self.latest_context.clone();
        let observed_events = self.observed_events.clone();
        let temporary_run_context = self.temporary_run_context.clone();
        match self.join().await {
            Ok(result) => {
                let events = result.events.clone();
                AgentStreamCompletion {
                    state: result.context.export_full_state(),
                    result: Some(result),
                    error: None,
                    events,
                }
            }
            Err(error) => {
                let mut state = latest_context.lock().await.export_full_state();
                restore_temporary_state_context(&mut state, temporary_run_context.as_ref());
                AgentStreamCompletion {
                    state,
                    result: None,
                    error: Some(error),
                    events: match observed_events.lock() {
                        Ok(events) => events.clone(),
                        Err(error) => error.into_inner().clone(),
                    },
                }
            }
        }
    }

    /// Wait for completion and write the final context back into a session.
    ///
    /// # Errors
    ///
    /// Propagates `join` errors.
    pub async fn finish_into_session(
        self,
        session: &mut AgentSession,
    ) -> Result<AgentLiveStreamResult, AgentStreamError> {
        let completion = self.complete().await;
        if let Some(result) = completion.result {
            session.replace_context(result.context.clone());
            session.record_result(&result.result);
            return Ok(result);
        }
        session.replace_context(AgentContext::from_state(completion.state));
        Err(completion.error.unwrap_or_else(|| {
            AgentStreamError::Join("stream completed without result or error".to_string())
        }))
    }
}

fn apply_temporary_run_context(
    result: &mut AgentLiveStreamResult,
    temporary: Option<&TemporaryRunContext>,
) {
    let Some(temporary) = temporary else {
        return;
    };
    if !temporary.trace_metadata.is_empty() {
        result.result.state.metadata.insert(
            TRACE_METADATA_STATE_KEY.to_string(),
            Value::Object(temporary.trace_metadata.clone()),
        );
    }
    for (key, value) in &temporary.context_metadata {
        result
            .result
            .state
            .metadata
            .insert(key.clone(), value.clone());
    }
    restore_context_overrides(&mut result.context, Some(temporary.restore.clone()));
}

fn restore_temporary_state_context(
    state: &mut ResumableState,
    temporary: Option<&TemporaryRunContext>,
) {
    if let Some(temporary) = temporary {
        if let Some(metadata) = &temporary.restore.metadata {
            state.metadata.clone_from(metadata);
        }
        if let Some(tool_config) = &temporary.restore.tool_config {
            state.tool_config.clone_from(tool_config);
        }
        if let Some(security) = &temporary.restore.security {
            state.security.clone_from(security);
        }
    }
}

fn restore_temporary_context(context: &mut AgentContext, temporary: Option<&TemporaryRunContext>) {
    if let Some(temporary) = temporary {
        restore_context_overrides(context, Some(temporary.restore.clone()));
    }
}

pub(crate) fn start_session_stream(
    agent: starweaver_runtime::Agent,
    context: AgentContext,
    prompt: AgentInput,
) -> AgentStreamHandle {
    match try_start_session_stream(agent, context, prompt) {
        Ok(handle) => handle,
        Err(error) => panic!("live agent streams require an active Tokio runtime: {error}"),
    }
}

pub(crate) fn try_start_session_stream(
    agent: starweaver_runtime::Agent,
    context: AgentContext,
    prompt: AgentInput,
) -> Result<AgentStreamHandle, AgentStreamError> {
    try_start_session_stream_with_options(agent, context, prompt, AgentStreamOptions::default())
}

pub(crate) fn start_session_stream_with_options(
    agent: starweaver_runtime::Agent,
    context: AgentContext,
    prompt: AgentInput,
    options: AgentStreamOptions,
) -> AgentStreamHandle {
    match try_start_session_stream_with_options(agent, context, prompt, options) {
        Ok(handle) => handle,
        Err(error) => panic!("live agent streams require an active Tokio runtime: {error}"),
    }
}

pub(crate) fn try_start_session_stream_with_options(
    agent: starweaver_runtime::Agent,
    context: AgentContext,
    prompt: AgentInput,
    options: AgentStreamOptions,
) -> Result<AgentStreamHandle, AgentStreamError> {
    let runtime = tokio::runtime::Handle::try_current()
        .map_err(|error| AgentStreamError::RuntimeUnavailable(error.to_string()))?;
    let options = options.normalized();
    let (sender, receiver) = mpsc::channel(options.buffer_size);
    let latest_context = Arc::new(tokio::sync::Mutex::new(context.clone()));
    let interrupted = Arc::new(AtomicBool::new(false));
    let interrupt_reason = Arc::new(Mutex::new(None));
    let cancellation_token = CancellationToken::new();
    let dropped_events = Arc::new(AtomicUsize::new(0));
    let receiver_closed = Arc::new(AtomicBool::new(false));
    let current_error = Arc::new(Mutex::new(None));
    let observed_events = Arc::new(Mutex::new(Vec::new()));
    let control = new_control_handle(
        interrupted.clone(),
        interrupt_reason.clone(),
        cancellation_token.clone(),
        latest_context.clone(),
    );
    let observer = Arc::new(LiveStreamObserver {
        sender,
        latest_context: latest_context.clone(),
        interrupted: interrupted.clone(),
        interrupt_reason,
        finished: control.finished.clone(),
        dropped_events: dropped_events.clone(),
        receiver_closed: receiver_closed.clone(),
        observed_events: observed_events.clone(),
        drop_policy: options.drop_policy,
    });
    let agent = agent
        .with_capability(control.drain_capability())
        .with_stream_observer(observer)
        .with_cancellation_token(cancellation_token);
    let join_latest_context = latest_context.clone();
    let join_current_error = current_error.clone();
    let join = runtime.spawn(async move {
        let mut context = context;
        let mut events = Vec::new();
        let result = agent
            .run_with_context_and_stream_events(prompt, &mut context, &mut events)
            .await;
        match result {
            Ok(result) => {
                *join_latest_context.lock().await = context.clone();
                Ok(AgentLiveStreamResult {
                    result,
                    context,
                    events,
                })
            }
            Err(error) => {
                let current_error = Some(match &error {
                    AgentError::Cancelled { .. } => AgentStreamCurrentError::Interrupted,
                    _ => AgentStreamCurrentError::Agent(error.to_string()),
                });
                match join_current_error.lock() {
                    Ok(mut error) => *error = current_error,
                    Err(error) => *error.into_inner() = current_error,
                }
                let repair_reason = match &error {
                    AgentError::Cancelled { reason } => reason.clone(),
                    _ => error.to_string(),
                };
                let repaired_tool_calls = context.repair_dangling_tool_calls(repair_reason);
                if repaired_tool_calls > 0 {
                    context.publish_event(AgentEvent::new(
                        "tool_calls_repaired",
                        serde_json::json!({
                            "run_id": context.run_id.as_ref().map(starweaver_core::RunId::as_str),
                            "repaired_tool_calls": repaired_tool_calls,
                        }),
                    ));
                }
                if let AgentError::Cancelled { reason } = &error {
                    context.publish_event(AgentEvent::new(
                        "run_cancelled",
                        serde_json::json!({
                            "run_id": context.run_id.as_ref().map(starweaver_core::RunId::as_str),
                            "reason": reason,
                        }),
                    ));
                    context.finish_run();
                }
                *join_latest_context.lock().await = context.clone();
                Err(error)
            }
        }
    });
    Ok(AgentStreamHandle::new(
        receiver,
        join,
        latest_context,
        interrupted,
        control,
        dropped_events,
        receiver_closed,
        current_error,
        observed_events,
        options,
    ))
}

struct LiveStreamObserver {
    sender: mpsc::Sender<AgentStreamRecord>,
    latest_context: Arc<tokio::sync::Mutex<AgentContext>>,
    interrupted: Arc<AtomicBool>,
    interrupt_reason: Arc<Mutex<Option<String>>>,
    finished: Arc<AtomicBool>,
    dropped_events: Arc<AtomicUsize>,
    receiver_closed: Arc<AtomicBool>,
    observed_events: Arc<Mutex<Vec<AgentStreamRecord>>>,
    drop_policy: AgentStreamDropPolicy,
}

#[async_trait]
impl AgentCapability for LiveStreamObserver {
    async fn on_stream_event_with_context(
        &self,
        _state: &AgentRunState,
        context: &AgentContext,
        event: &AgentStreamRecord,
    ) -> CapabilityResult<()> {
        *self.latest_context.lock().await = context.clone();
        if matches!(
            event.event,
            AgentStreamEvent::RunComplete { .. }
                | AgentStreamEvent::RunFailed { .. }
                | AgentStreamEvent::Suspended { .. }
        ) {
            self.finished.store(true, Ordering::SeqCst);
        }
        match self.observed_events.lock() {
            Ok(mut events) => events.push(event.clone()),
            Err(error) => error.into_inner().push(event.clone()),
        }
        if self.interrupted.load(Ordering::SeqCst) {
            return Err(CapabilityError::Cancelled {
                reason: interrupt_reason(&self.interrupt_reason),
            });
        }
        if self.receiver_closed.load(Ordering::SeqCst) {
            return Ok(());
        }
        match self.drop_policy {
            AgentStreamDropPolicy::DropNewest => match self.sender.try_send(event.clone()) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) => {
                    self.dropped_events.fetch_add(1, Ordering::SeqCst);
                }
                Err(TrySendError::Closed(_)) => {
                    self.receiver_closed.store(true, Ordering::SeqCst);
                }
            },
            AgentStreamDropPolicy::Backpressure => {
                if self.sender.send(event.clone()).await.is_err() {
                    self.receiver_closed.store(true, Ordering::SeqCst);
                }
            }
        }
        Ok(())
    }
}

fn interrupt_reason(reason: &Mutex<Option<String>>) -> String {
    match reason.lock() {
        Ok(reason) => reason
            .clone()
            .unwrap_or_else(|| "agent stream interruption requested".to_string()),
        Err(error) => error
            .into_inner()
            .clone()
            .unwrap_or_else(|| "agent stream interruption requested".to_string()),
    }
}

fn map_stream_join_result(
    result: Result<Result<AgentLiveStreamResult, AgentError>, JoinError>,
) -> Result<AgentLiveStreamResult, AgentStreamError> {
    match result {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(AgentError::Cancelled { reason })) => Err(AgentStreamError::Interrupted { reason }),
        Ok(Err(error)) => Err(AgentStreamError::Agent(error)),
        Err(error) if error.is_cancelled() => Err(AgentStreamError::Interrupted {
            reason: "agent stream task was cancelled".to_string(),
        }),
        Err(error) => Err(AgentStreamError::Join(error.to_string())),
    }
}
