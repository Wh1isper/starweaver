//! Live SDK streaming helpers.

use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use async_trait::async_trait;
use starweaver_context::{AgentContext, AgentEvent, ResumableState};
use starweaver_core::CancellationToken;
use starweaver_runtime::{
    AgentCapability, AgentError, AgentInput, AgentResult, AgentRunState, AgentStreamRecord,
    AgentStreamResult, CapabilityError, CapabilityResult,
};
use thiserror::Error;
use tokio::{
    sync::mpsc::{self, error::TrySendError},
    task::{JoinError, JoinHandle},
};

use crate::AgentSession;

const DEFAULT_STREAM_BUFFER: usize = 256;
const INTERRUPT_JOIN_TIMEOUT: Duration = Duration::from_secs(5);

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

/// Error returned by live stream handles.
#[derive(Debug, Error)]
pub enum AgentStreamError {
    /// Live stream construction was attempted without an active Tokio runtime.
    #[error("tokio runtime unavailable for live agent stream: {0}")]
    RuntimeUnavailable(String),
    /// The caller deliberately interrupted the live stream.
    #[error("agent stream interrupted")]
    Interrupted,
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
    cancellation_token: CancellationToken,
    dropped_events: Arc<AtomicUsize>,
    receiver_closed: Arc<AtomicBool>,
    current_error: Arc<Mutex<Option<AgentStreamCurrentError>>>,
    observed_events: Arc<Mutex<Vec<AgentStreamRecord>>>,
    options: AgentStreamOptions,
}

impl AgentStreamHandle {
    #[allow(clippy::too_many_arguments)]
    pub(crate) const fn new(
        receiver: mpsc::Receiver<AgentStreamRecord>,
        join: JoinHandle<Result<AgentLiveStreamResult, AgentError>>,
        latest_context: Arc<tokio::sync::Mutex<AgentContext>>,
        interrupted: Arc<AtomicBool>,
        cancellation_token: CancellationToken,
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
            cancellation_token,
            dropped_events,
            receiver_closed,
            current_error,
            observed_events,
            options,
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

    /// Request cooperative cancellation of the running stream.
    ///
    /// The producer observes this flag at stream/capability boundaries and exits with
    /// `AgentStreamError::Interrupted`. `join` still has a timeout-backed abort fallback
    /// for models or tools that never yield another boundary.
    pub fn interrupt(&self) {
        self.interrupted.store(true, Ordering::SeqCst);
        self.cancellation_token.cancel();
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

    /// Return the latest observed context clone.
    pub async fn latest_context(&self) -> AgentContext {
        self.latest_context.lock().await.clone()
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
        if interrupted.load(Ordering::SeqCst) {
            if let Ok(result) = tokio::time::timeout(INTERRUPT_JOIN_TIMEOUT, &mut join).await {
                return map_stream_join_result(result);
            }
            join.abort();
            let _ = join.await;
            return Err(AgentStreamError::Interrupted);
        }
        map_stream_join_result(join.await)
    }

    /// Wait for stream completion and return result, error, and recoverable state
    /// without propagating failures.
    pub async fn complete(self) -> AgentStreamCompletion {
        let latest_context = self.latest_context.clone();
        let observed_events = self.observed_events.clone();
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
            Err(error) => AgentStreamCompletion {
                state: latest_context.lock().await.export_full_state(),
                result: None,
                error: Some(error),
                events: match observed_events.lock() {
                    Ok(events) => events.clone(),
                    Err(error) => error.into_inner().clone(),
                },
            },
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
        let result = self.join().await?;
        session.replace_context(result.context.clone());
        session.record_result(&result.result);
        Ok(result)
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
    let cancellation_token = CancellationToken::new();
    let dropped_events = Arc::new(AtomicUsize::new(0));
    let receiver_closed = Arc::new(AtomicBool::new(false));
    let current_error = Arc::new(Mutex::new(None));
    let observed_events = Arc::new(Mutex::new(Vec::new()));
    let observer = Arc::new(LiveStreamObserver {
        sender,
        latest_context: latest_context.clone(),
        interrupted: interrupted.clone(),
        dropped_events: dropped_events.clone(),
        receiver_closed: receiver_closed.clone(),
        observed_events: observed_events.clone(),
        drop_policy: options.drop_policy,
    });
    let agent = agent
        .with_stream_observer(observer)
        .with_cancellation_token(cancellation_token.clone());
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
        cancellation_token,
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
        match self.observed_events.lock() {
            Ok(mut events) => events.push(event.clone()),
            Err(error) => error.into_inner().push(event.clone()),
        }
        if self.interrupted.load(Ordering::SeqCst) {
            return Err(CapabilityError::Cancelled {
                reason: "agent stream interruption requested".to_string(),
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

fn map_stream_join_result(
    result: Result<Result<AgentLiveStreamResult, AgentError>, JoinError>,
) -> Result<AgentLiveStreamResult, AgentStreamError> {
    match result {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(AgentError::Cancelled { .. })) => Err(AgentStreamError::Interrupted),
        Ok(Err(error)) => Err(AgentStreamError::Agent(error)),
        Err(error) if error.is_cancelled() => Err(AgentStreamError::Interrupted),
        Err(error) => Err(AgentStreamError::Join(error.to_string())),
    }
}
