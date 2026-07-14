//! Host-owned lifecycle supervisor for asynchronous subagent attempts.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{Arc, Mutex},
    time::Duration,
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use starweaver_context::{AgentContext, AgentEvent, AgentInfo, BusMessage};
use starweaver_core::{CancellationToken, RunId, SessionId, SubagentAttemptId, TaskId};
use starweaver_model::ModelMessage;
use starweaver_usage::{Usage, UsageSnapshotEntry};
use tokio::task::{AbortHandle, JoinHandle};

const DEFAULT_MAX_ACTIVE_ATTEMPTS: usize = 8;
const DEFAULT_MAX_RETAINED_RESULTS: usize = 128;
const DEFAULT_MAX_STEERING_MESSAGES: usize = 32;
const DEFAULT_MAX_STEERING_BYTES: usize = 16 * 1024;
const DEFAULT_MAX_PROMPT_BYTES: usize = 64 * 1024;
const DEFAULT_MAX_CANCELLATION_REASON_BYTES: usize = 1024;
const DEFAULT_MAX_RESULT_BYTES: usize = 64 * 1024;
const DEFAULT_MAX_OPERATION_ID_BYTES: usize = 256;
const DEFAULT_MAX_OPERATION_IDS: usize = 256;
const DEFAULT_SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// Host limits applied by one background-subagent supervisor scope.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackgroundSubagentLimits {
    /// Maximum concurrently non-terminal attempts.
    pub max_active_attempts: usize,
    /// Maximum terminal result records retained in memory.
    pub max_retained_results: usize,
    /// Maximum queued steering messages for one attempt.
    pub max_steering_messages: usize,
    /// Maximum UTF-8 bytes accepted in one steering message.
    pub max_steering_bytes: usize,
    /// Maximum UTF-8 bytes accepted in one delegated prompt.
    pub max_prompt_bytes: usize,
    /// Maximum UTF-8 bytes retained for a safe cancellation reason.
    pub max_cancellation_reason_bytes: usize,
    /// Maximum successful result/error bytes retained inline.
    pub max_inline_result_bytes: usize,
    /// Maximum UTF-8 bytes accepted in an idempotency operation id.
    pub max_operation_id_bytes: usize,
    /// Maximum steering/cancellation operation ids retained per attempt.
    pub max_operation_ids_per_attempt: usize,
    /// Cooperative cancellation grace period before task abort.
    pub cancellation_grace: Duration,
    /// Grace period used by default shutdown.
    pub shutdown_grace: Duration,
}

impl Default for BackgroundSubagentLimits {
    fn default() -> Self {
        Self {
            max_active_attempts: DEFAULT_MAX_ACTIVE_ATTEMPTS,
            max_retained_results: DEFAULT_MAX_RETAINED_RESULTS,
            max_steering_messages: DEFAULT_MAX_STEERING_MESSAGES,
            max_steering_bytes: DEFAULT_MAX_STEERING_BYTES,
            max_prompt_bytes: DEFAULT_MAX_PROMPT_BYTES,
            max_cancellation_reason_bytes: DEFAULT_MAX_CANCELLATION_REASON_BYTES,
            max_inline_result_bytes: DEFAULT_MAX_RESULT_BYTES,
            max_operation_id_bytes: DEFAULT_MAX_OPERATION_ID_BYTES,
            max_operation_ids_per_attempt: DEFAULT_MAX_OPERATION_IDS,
            cancellation_grace: DEFAULT_SHUTDOWN_GRACE,
            shutdown_grace: DEFAULT_SHUTDOWN_GRACE,
        }
    }
}

/// Monotonic execution state for one subagent attempt.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundSubagentExecutionStatus {
    /// Identity, quota, and ownership have been accepted.
    Accepted,
    /// Child construction is in progress.
    Starting,
    /// Child execution can make progress.
    Running,
    /// The same attempt is waiting for an explicitly resumable condition.
    Waiting,
    /// Child execution completed successfully.
    Completed,
    /// Child execution failed.
    Failed,
    /// Child execution was cancelled or interrupted.
    Cancelled,
}

impl BackgroundSubagentExecutionStatus {
    /// Return the stable serialized name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Waiting => "waiting",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    /// Return whether this is an immutable terminal outcome.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

/// Backward-compatible terminal status name.
pub type BackgroundSubagentTaskStatus = BackgroundSubagentExecutionStatus;

/// Orthogonal logical result-delivery state.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundSubagentDeliveryStatus {
    /// No consumer owns delivery.
    #[default]
    Undelivered,
    /// One consumer atomically owns delivery under a claim.
    Claimed,
    /// A parent turn, explicit wait, or continuation consumed the result.
    Delivered,
}

/// Retention state for terminal content, independent from execution/delivery.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundSubagentRetentionStatus {
    /// Bounded content is retained inline.
    #[default]
    Inline,
    /// Content moved to a host-owned artifact reference.
    Artifact,
    /// Volatile content expired while minimal audit evidence remains.
    Expired,
}

/// Atomic delivery claim for one terminal result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BackgroundSubagentDeliveryClaim {
    /// Stable consumer-generated claim id.
    pub claim_id: String,
    /// Optional continuation run admitted under this claim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation_run_id: Option<RunId>,
    /// Claim deadline after which reconciliation may release it.
    pub deadline: DateTime<Utc>,
}

/// Snapshot of one accepted background subagent attempt.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BackgroundSubagentTaskInfo {
    /// Unique execution-attempt identity.
    pub attempt_id: SubagentAttemptId,
    /// Stable subagent conversation identity.
    pub agent_id: String,
    /// Registered subagent name.
    pub subagent_name: String,
    /// Optional task-bundle work item linked to this attempt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked_task_id: Option<TaskId>,
    /// Current monotonic execution state.
    pub execution_status: BackgroundSubagentExecutionStatus,
    /// Parent durable session when one exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<SessionId>,
    /// Parent run that accepted the delegation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_run_id: Option<RunId>,
    /// Child runtime run once known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_run_id: Option<RunId>,
    /// Whether this attempt continues a prior terminal conversation.
    pub is_resume: bool,
    /// Bounded prompt preview retained for host diagnostics.
    pub prompt_preview: String,
    /// Acceptance timestamp.
    pub accepted_at: DateTime<Utc>,
    /// Last state-change timestamp.
    pub updated_at: DateTime<Utc>,
}

/// Cached terminal evidence for one background subagent attempt.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BackgroundSubagentTaskResult {
    /// Unique execution-attempt identity.
    pub attempt_id: SubagentAttemptId,
    /// Stable conversation identity.
    pub agent_id: String,
    /// Registered subagent name.
    pub subagent_name: String,
    /// Optional task-bundle work item.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked_task_id: Option<TaskId>,
    /// Immutable terminal execution outcome.
    pub status: BackgroundSubagentExecutionStatus,
    /// Parent durable session when one exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<SessionId>,
    /// Parent run that accepted this attempt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_run_id: Option<RunId>,
    /// Child runtime run once known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_run_id: Option<RunId>,
    /// Successful bounded content, when retained inline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Safe bounded failure message, when retained inline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Safe failure/interruption category without raw error content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_category: Option<String>,
    /// Optional bounded cancellation reason supplied by the owner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancellation_reason: Option<String>,
    /// Logical delivery state.
    pub delivery_status: BackgroundSubagentDeliveryStatus,
    /// Current delivery claim, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery_claim: Option<BackgroundSubagentDeliveryClaim>,
    /// Claim id that completed delivery, retained for idempotent acknowledgement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivered_claim_id: Option<String>,
    /// Content-retention state.
    pub retention_status: BackgroundSubagentRetentionStatus,
    /// Completion timestamp.
    pub completed_at: DateTime<Utc>,
}

impl BackgroundSubagentTaskResult {
    pub(crate) fn terminal(
        info: &BackgroundSubagentTaskInfo,
        status: BackgroundSubagentExecutionStatus,
        content: Option<String>,
        error: Option<String>,
        max_inline_bytes: usize,
    ) -> Self {
        debug_assert!(status.is_terminal());
        let content = content.map(|value| bounded_text(&value, max_inline_bytes));
        let error = error.map(|value| bounded_text(&value, max_inline_bytes));
        Self {
            attempt_id: info.attempt_id.clone(),
            agent_id: info.agent_id.clone(),
            subagent_name: info.subagent_name.clone(),
            linked_task_id: info.linked_task_id.clone(),
            status,
            parent_session_id: info.parent_session_id.clone(),
            parent_run_id: info.parent_run_id.clone(),
            child_run_id: info.child_run_id.clone(),
            content,
            error,
            failure_category: match status {
                BackgroundSubagentExecutionStatus::Failed => Some("execution_error".to_string()),
                BackgroundSubagentExecutionStatus::Cancelled => Some("cancelled".to_string()),
                _ => None,
            },
            cancellation_reason: None,
            delivery_status: BackgroundSubagentDeliveryStatus::Undelivered,
            delivery_claim: None,
            delivered_claim_id: None,
            retention_status: BackgroundSubagentRetentionStatus::Inline,
            completed_at: Utc::now(),
        }
    }
}

/// Receipt for accepted steering of one active attempt.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BackgroundSubagentSteeringReceipt {
    /// Target attempt.
    pub attempt_id: SubagentAttemptId,
    /// Stable conversation identity.
    pub agent_id: String,
    /// Idempotent steering operation id.
    pub steering_id: String,
    /// Stable receipt state.
    pub status: String,
}

/// Receipt for cooperative cancellation of one active attempt.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BackgroundSubagentCancellationReceipt {
    /// Target attempt.
    pub attempt_id: SubagentAttemptId,
    /// Stable conversation identity.
    pub agent_id: String,
    /// Idempotent cancellation operation id.
    pub cancellation_id: String,
    /// Stable receipt state or terminal outcome.
    pub status: String,
}

/// Safe typed supervisor error.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum BackgroundSubagentError {
    /// Supervisor is shutting down and accepts no new work.
    #[error("background subagent supervisor is shutting down")]
    Closing,
    /// Active-attempt quota is exhausted.
    #[error("background subagent quota exceeded")]
    QuotaExceeded,
    /// Another non-terminal attempt already owns this conversation.
    #[error("subagent conversation already has an active attempt")]
    ActiveConversation,
    /// Target attempt is unknown in this supervisor scope.
    #[error("background subagent attempt not found")]
    NotFound,
    /// Target attempt is terminal.
    #[error("background subagent attempt is terminal")]
    Terminal,
    /// Delegated prompt exceeded configured bounds.
    #[error("background subagent prompt exceeds configured bounds")]
    PromptTooLarge,
    /// Steering payload exceeded configured bounds.
    #[error("background subagent steering payload exceeds configured bounds")]
    SteeringTooLarge,
    /// Cancellation reason exceeded configured bounds.
    #[error("background subagent cancellation reason exceeds configured bounds")]
    CancellationReasonTooLarge,
    /// Steering queue is full.
    #[error("background subagent steering queue is full")]
    SteeringQueueFull,
    /// Caller-supplied idempotency operation id exceeded configured bounds.
    #[error("background subagent operation id exceeds configured bounds")]
    OperationIdTooLarge,
    /// Per-attempt idempotency history is full.
    #[error("background subagent operation history is full")]
    OperationHistoryFull,
    /// Delivery is already claimed by another consumer.
    #[error("background subagent result delivery is already claimed")]
    DeliveryClaimed,
    /// Result was already logically delivered.
    #[error("background subagent result was already delivered")]
    Delivered,
}

/// Synchronous host callback invoked after terminal evidence is visible.
pub trait BackgroundSubagentCompletionCallback: Send + Sync {
    /// Notify the host that one terminal result may require a continuation.
    fn on_completion(&self, result: &BackgroundSubagentTaskResult);
}

impl<F> BackgroundSubagentCompletionCallback for F
where
    F: Fn(&BackgroundSubagentTaskResult) + Send + Sync,
{
    fn on_completion(&self, result: &BackgroundSubagentTaskResult) {
        self(result);
    }
}

#[derive(Clone)]
pub(super) struct BackgroundSubagentChildControl {
    pub cancellation: CancellationToken,
    pub pending_messages: Arc<tokio::sync::Mutex<VecDeque<BusMessage>>>,
}

#[derive(Clone)]
struct ActiveAttempt {
    info: BackgroundSubagentTaskInfo,
    control: BackgroundSubagentChildControl,
    abort_handle: Option<AbortHandle>,
    steering_ids: BTreeSet<String>,
    cancellation_ids: BTreeSet<String>,
    cancellation_reason: Option<String>,
    cancellation_timer_started: bool,
}

#[derive(Clone)]
pub(super) struct BackgroundSubagentContextDelta {
    usage: Usage,
    usage_snapshot_entries: BTreeMap<String, UsageSnapshotEntry>,
    agent_registry: BTreeMap<String, AgentInfo>,
    subagent_history: BTreeMap<String, Vec<ModelMessage>>,
    events: Vec<AgentEvent>,
}

impl BackgroundSubagentContextDelta {
    pub(crate) fn from_context(
        source: &AgentContext,
        base_usage: &Usage,
        base_usage_snapshot_keys: &BTreeSet<String>,
        base_event_count: usize,
        agent_id: &str,
    ) -> Self {
        Self {
            usage: Usage {
                requests: source.usage.requests.saturating_sub(base_usage.requests),
                input_tokens: source
                    .usage
                    .input_tokens
                    .saturating_sub(base_usage.input_tokens),
                cache_write_tokens: source
                    .usage
                    .cache_write_tokens
                    .saturating_sub(base_usage.cache_write_tokens),
                cache_read_tokens: source
                    .usage
                    .cache_read_tokens
                    .saturating_sub(base_usage.cache_read_tokens),
                output_tokens: source
                    .usage
                    .output_tokens
                    .saturating_sub(base_usage.output_tokens),
                total_tokens: source
                    .usage
                    .total_tokens
                    .saturating_sub(base_usage.total_tokens),
                tool_calls: source
                    .usage
                    .tool_calls
                    .saturating_sub(base_usage.tool_calls),
            },
            usage_snapshot_entries: source
                .usage_snapshot_entries
                .iter()
                .filter(|(key, _)| !base_usage_snapshot_keys.contains(*key))
                .map(|(key, entry)| (key.clone(), entry.clone()))
                .collect(),
            agent_registry: source
                .agent_registry
                .get(agent_id)
                .map(|info| BTreeMap::from([(agent_id.to_string(), info.clone())]))
                .unwrap_or_default(),
            subagent_history: source
                .subagent_history
                .get(agent_id)
                .map(|history| BTreeMap::from([(agent_id.to_string(), history.clone())]))
                .unwrap_or_default(),
            events: source
                .events
                .events()
                .iter()
                .skip(base_event_count)
                .cloned()
                .collect(),
        }
    }

    fn apply(&self, attempt_id: &SubagentAttemptId, target: &mut AgentContext) {
        let operation_id = attempt_id.as_str().to_string();
        if target
            .tools
            .background_context_delta_ids
            .contains(&operation_id)
        {
            return;
        }
        target.usage.add_assign(&self.usage);
        for (key, entry) in &self.usage_snapshot_entries {
            target
                .usage_snapshot_entries
                .entry(key.clone())
                .or_insert_with(|| entry.clone());
        }
        for (agent_id, info) in &self.agent_registry {
            target.agent_registry.insert(agent_id.clone(), info.clone());
        }
        for (agent_id, history) in &self.subagent_history {
            target
                .subagent_history
                .entry(agent_id.clone())
                .or_default()
                .extend(history.iter().cloned());
        }
        for event in &self.events {
            target.events.publish(event.clone());
        }
        target
            .tools
            .background_context_delta_ids
            .insert(operation_id);
    }
}

#[derive(Clone)]
struct PendingResultMessage {
    attempt_id: SubagentAttemptId,
    message: BusMessage,
}

pub(super) struct ClaimedBackgroundSubagentMessage {
    pub attempt_id: SubagentAttemptId,
    pub claim_id: String,
    pub message: BusMessage,
}

pub(super) struct BackgroundSubagentAcceptance {
    pub attempt_id: SubagentAttemptId,
    pub agent_id: String,
    pub subagent_name: String,
    pub linked_task_id: Option<TaskId>,
    pub prompt: String,
    pub parent_session_id: Option<SessionId>,
    pub parent_run_id: Option<RunId>,
    pub is_resume: bool,
}

#[derive(Default)]
struct BackgroundSubagentState {
    closing: bool,
    active: BTreeMap<SubagentAttemptId, ActiveAttempt>,
    active_by_agent: BTreeMap<String, SubagentAttemptId>,
    conversations: BTreeMap<String, String>,
    results: BTreeMap<SubagentAttemptId, BackgroundSubagentTaskResult>,
    parent_liveness_enabled: bool,
    active_parent_runs: BTreeSet<RunId>,
    waiting_attempts: BTreeSet<SubagentAttemptId>,
    context_deltas: BTreeMap<SubagentAttemptId, BackgroundSubagentContextDelta>,
    pending_messages: VecDeque<PendingResultMessage>,
    finalizer_handles: BTreeMap<SubagentAttemptId, JoinHandle<()>>,
}

/// Host-owned supervisor for asynchronous subagent attempts.
///
/// The supervisor is intentionally injectable and may outlive individual parent
/// runtimes. It owns cancellation/abort handles, attempt-keyed terminal evidence,
/// delivery claims, bounded retention, and pending parent messages.
pub struct BackgroundSubagentSupervisor {
    state: Mutex<BackgroundSubagentState>,
    notify: tokio::sync::Notify,
    limits: BackgroundSubagentLimits,
    completion_callback: Option<Arc<dyn BackgroundSubagentCompletionCallback>>,
}

/// Backward-compatible name retained for 0.x callers.
pub type BackgroundSubagentMonitor = BackgroundSubagentSupervisor;

impl Default for BackgroundSubagentSupervisor {
    fn default() -> Self {
        Self::new()
    }
}

impl BackgroundSubagentSupervisor {
    /// Create an empty supervisor with bounded defaults.
    #[must_use]
    pub fn new() -> Self {
        Self::with_limits(BackgroundSubagentLimits::default())
    }

    /// Create an empty supervisor with explicit host limits.
    #[must_use]
    pub fn with_limits(limits: BackgroundSubagentLimits) -> Self {
        Self {
            state: Mutex::new(BackgroundSubagentState::default()),
            notify: tokio::sync::Notify::new(),
            limits,
            completion_callback: None,
        }
    }

    /// Install a host completion callback.
    #[must_use]
    pub fn with_completion_callback(
        mut self,
        callback: Arc<dyn BackgroundSubagentCompletionCallback>,
    ) -> Self {
        self.completion_callback = Some(callback);
        self
    }

    /// Return configured limits.
    #[must_use]
    pub const fn limits(&self) -> &BackgroundSubagentLimits {
        &self.limits
    }

    fn with_state<R>(&self, f: impl FnOnce(&mut BackgroundSubagentState) -> R) -> R {
        match self.state.lock() {
            Ok(mut state) => f(&mut state),
            Err(error) => f(&mut error.into_inner()),
        }
    }

    /// Mark one parent run active for direct in-turn result delivery.
    ///
    /// Calling this method enables explicit parent-liveness tracking for the
    /// supervisor scope. Hosts must pair it with [`Self::end_parent_run`].
    pub fn begin_parent_run(&self, run_id: RunId) {
        self.with_state(|state| {
            state.parent_liveness_enabled = true;
            state.active_parent_runs.insert(run_id);
        });
    }

    /// Mark one tracked parent run idle or terminal.
    pub fn end_parent_run(&self, run_id: &RunId) {
        self.with_state(|state| {
            state.parent_liveness_enabled = true;
            state.active_parent_runs.remove(run_id);
        });
    }

    /// Return whether an attempt's parent run is currently active.
    #[must_use]
    pub fn parent_run_is_active(&self, attempt_id: &SubagentAttemptId) -> bool {
        self.with_state(|state| {
            if !state.parent_liveness_enabled {
                return true;
            }
            state
                .active
                .get(attempt_id)
                .and_then(|active| active.info.parent_run_id.as_ref())
                .is_some_and(|run_id| state.active_parent_runs.contains(run_id))
                || state
                    .results
                    .get(attempt_id)
                    .and_then(|result| result.parent_run_id.as_ref())
                    .is_some_and(|run_id| state.active_parent_runs.contains(run_id))
        })
    }

    /// Return whether the SDK compatibility path may deliver directly into a
    /// live context. Explicitly tracked product hosts use callback/fallback
    /// delivery so terminal output races cannot lose a wake-up.
    #[must_use]
    pub fn direct_delivery_allowed(&self, attempt_id: &SubagentAttemptId) -> bool {
        self.with_state(|state| {
            !state.parent_liveness_enabled
                && (state.active.contains_key(attempt_id) || state.results.contains_key(attempt_id))
        })
    }

    pub(super) fn merge_or_stage_context_delta(
        &self,
        attempt_id: &SubagentAttemptId,
        context_handle: &starweaver_context::AgentContextHandle,
        delta: BackgroundSubagentContextDelta,
    ) {
        if self.parent_run_is_active(attempt_id) {
            context_handle.update(|context| delta.apply(attempt_id, context));
        }
        self.with_state(|state| {
            state.context_deltas.insert(attempt_id.clone(), delta);
        });
    }

    pub(crate) fn apply_context_deltas(&self, context: &mut AgentContext) {
        let deltas = self.with_state(|state| std::mem::take(&mut state.context_deltas));
        for (attempt_id, delta) in deltas {
            delta.apply(&attempt_id, context);
        }
        self.with_state(|state| trim_results(state, self.limits.max_retained_results));
    }

    /// Return whether a prior or active conversation identity is known.
    #[must_use]
    pub fn knows_conversation(&self, agent_id: &str, subagent_name: &str) -> bool {
        self.with_state(|state| {
            state
                .conversations
                .get(agent_id)
                .is_some_and(|known| known == subagent_name)
        })
    }

    /// Atomically reserve one accepted attempt and active conversation slot.
    pub(super) fn accept(
        &self,
        acceptance: BackgroundSubagentAcceptance,
    ) -> Result<BackgroundSubagentTaskInfo, BackgroundSubagentError> {
        if acceptance.prompt.len() > self.limits.max_prompt_bytes {
            return Err(BackgroundSubagentError::PromptTooLarge);
        }
        let BackgroundSubagentAcceptance {
            attempt_id,
            agent_id,
            subagent_name,
            linked_task_id,
            prompt,
            parent_session_id,
            parent_run_id,
            is_resume,
        } = acceptance;
        let now = Utc::now();
        let info = BackgroundSubagentTaskInfo {
            attempt_id: attempt_id.clone(),
            agent_id: agent_id.clone(),
            subagent_name: subagent_name.clone(),
            linked_task_id,
            execution_status: BackgroundSubagentExecutionStatus::Accepted,
            parent_session_id,
            parent_run_id,
            child_run_id: None,
            is_resume,
            prompt_preview: bounded_text(&prompt, 240),
            accepted_at: now,
            updated_at: now,
        };
        let control = BackgroundSubagentChildControl {
            cancellation: CancellationToken::new(),
            pending_messages: Arc::new(tokio::sync::Mutex::new(VecDeque::new())),
        };
        self.with_state(|state| {
            if state.closing {
                return Err(BackgroundSubagentError::Closing);
            }
            if state.active.len() >= self.limits.max_active_attempts
                || state
                    .results
                    .values()
                    .filter(|result| {
                        result.delivery_status != BackgroundSubagentDeliveryStatus::Delivered
                    })
                    .count()
                    >= self.limits.max_retained_results
            {
                return Err(BackgroundSubagentError::QuotaExceeded);
            }
            if state.active_by_agent.contains_key(&agent_id) {
                return Err(BackgroundSubagentError::ActiveConversation);
            }
            state
                .conversations
                .insert(agent_id.clone(), subagent_name.clone());
            state.active_by_agent.insert(agent_id, attempt_id.clone());
            state.active.insert(
                attempt_id,
                ActiveAttempt {
                    info: info.clone(),
                    control,
                    abort_handle: None,
                    steering_ids: BTreeSet::new(),
                    cancellation_ids: BTreeSet::new(),
                    cancellation_reason: None,
                    cancellation_timer_started: false,
                },
            );
            Ok(info.clone())
        })?;
        self.notify.notify_waiters();
        Ok(info)
    }

    pub(super) fn child_control(
        &self,
        attempt_id: &SubagentAttemptId,
    ) -> Option<BackgroundSubagentChildControl> {
        self.with_state(|state| {
            state
                .active
                .get(attempt_id)
                .map(|active| active.control.clone())
        })
    }

    pub(super) fn attach_finalizer_handle(
        &self,
        attempt_id: &SubagentAttemptId,
        handle: JoinHandle<()>,
    ) {
        let mut handle = Some(handle);
        self.with_state(|state| {
            if state.active.contains_key(attempt_id)
                && let Some(handle) = handle.take()
            {
                state.finalizer_handles.insert(attempt_id.clone(), handle);
            }
        });
    }

    pub(super) fn detach_finalizer_handle(&self, attempt_id: &SubagentAttemptId) {
        self.with_state(|state| {
            state.finalizer_handles.remove(attempt_id);
        });
    }

    pub(crate) fn attach_abort_handle(
        &self,
        attempt_id: &SubagentAttemptId,
        abort_handle: AbortHandle,
    ) {
        let cancelled = self.with_state(|state| {
            let Some(active) = state.active.get_mut(attempt_id) else {
                return false;
            };
            let cancelled = active.control.cancellation.is_cancelled();
            active.abort_handle = Some(abort_handle.clone());
            cancelled
        });
        if cancelled {
            let grace = self.limits.cancellation_grace;
            tokio::spawn(async move {
                tokio::time::sleep(grace).await;
                abort_handle.abort();
            });
        }
    }

    pub(crate) fn transition(
        &self,
        attempt_id: &SubagentAttemptId,
        status: BackgroundSubagentExecutionStatus,
    ) {
        self.with_state(|state| {
            if let Some(active) = state.active.get_mut(attempt_id) {
                active.info.execution_status = status;
                active.info.updated_at = Utc::now();
            }
        });
        self.notify.notify_waiters();
    }

    pub(crate) fn set_child_run_id(
        &self,
        attempt_id: &SubagentAttemptId,
        child_run_id: Option<RunId>,
    ) {
        self.with_state(|state| {
            if let Some(active) = state.active.get_mut(attempt_id) {
                active.info.child_run_id = child_run_id;
                active.info.updated_at = Utc::now();
            }
        });
    }

    /// Return active background attempts in stable attempt-id order.
    #[must_use]
    pub fn active_tasks(&self) -> Vec<BackgroundSubagentTaskInfo> {
        self.with_state(|state| {
            state
                .active
                .values()
                .map(|active| active.info.clone())
                .collect()
        })
    }

    /// Return whether any background attempt is non-terminal.
    #[must_use]
    pub fn has_active_tasks(&self) -> bool {
        self.with_state(|state| !state.active.is_empty())
    }

    /// Return retained terminal results keyed by attempt id.
    #[must_use]
    pub fn task_results(&self) -> BTreeMap<SubagentAttemptId, BackgroundSubagentTaskResult> {
        self.with_state(|state| state.results.clone())
    }

    /// Return bounded known attempt ids.
    #[must_use]
    pub fn known_task_ids(&self) -> Vec<SubagentAttemptId> {
        self.with_state(|state| {
            state
                .active
                .keys()
                .chain(state.results.keys())
                .cloned()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .take(
                    self.limits
                        .max_retained_results
                        .saturating_add(self.limits.max_active_attempts),
                )
                .collect()
        })
    }

    /// Return one retained terminal result.
    #[must_use]
    pub fn task_result(
        &self,
        attempt_id: &SubagentAttemptId,
    ) -> Option<BackgroundSubagentTaskResult> {
        self.with_state(|state| state.results.get(attempt_id).cloned())
    }

    pub(crate) fn record_terminal(
        &self,
        attempt_id: &SubagentAttemptId,
        status: BackgroundSubagentExecutionStatus,
        content: Option<String>,
        error: Option<String>,
    ) -> Option<BackgroundSubagentTaskResult> {
        let result = self.with_state(|state| {
            if let Some(existing) = state.results.get(attempt_id) {
                return Some(existing.clone());
            }
            let active = state.active.remove(attempt_id)?;
            state.active_by_agent.remove(&active.info.agent_id);
            let mut result = BackgroundSubagentTaskResult::terminal(
                &active.info,
                status,
                content,
                error,
                self.limits.max_inline_result_bytes,
            );
            result.cancellation_reason = active.cancellation_reason;
            state.results.insert(attempt_id.clone(), result.clone());
            trim_results(state, self.limits.max_retained_results);
            Some(result)
        });
        self.notify.notify_waiters();
        result
    }

    pub(crate) fn notify_completion(&self, attempt_id: &SubagentAttemptId) {
        let result = self.task_result(attempt_id);
        if let Some(result) = result
            && result.delivery_status == BackgroundSubagentDeliveryStatus::Undelivered
            && let Some(callback) = self.completion_callback.as_ref()
        {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                callback.on_completion(&result);
            }));
        }
    }

    /// Queue idempotent steering for one active owned attempt.
    ///
    /// # Errors
    ///
    /// Returns an ownership, lifecycle, size, or queue-capacity error when the
    /// attempt cannot accept the steering message.
    pub async fn steer(
        &self,
        attempt_id: &SubagentAttemptId,
        message: String,
        steering_id: String,
    ) -> Result<BackgroundSubagentSteeringReceipt, BackgroundSubagentError> {
        if message.len() > self.limits.max_steering_bytes {
            return Err(BackgroundSubagentError::SteeringTooLarge);
        }
        if steering_id.len() > self.limits.max_operation_id_bytes {
            return Err(BackgroundSubagentError::OperationIdTooLarge);
        }
        let (agent_id, pending, duplicate) = self.with_state(|state| {
            let active = state
                .active
                .get_mut(attempt_id)
                .ok_or(BackgroundSubagentError::NotFound)?;
            let duplicate = active.steering_ids.contains(&steering_id);
            if !duplicate && active.steering_ids.len() >= self.limits.max_operation_ids_per_attempt
            {
                return Err(BackgroundSubagentError::OperationHistoryFull);
            }
            active.steering_ids.insert(steering_id.clone());
            Ok::<_, BackgroundSubagentError>((
                active.info.agent_id.clone(),
                active.control.pending_messages.clone(),
                duplicate,
            ))
        })?;
        if duplicate {
            return Ok(BackgroundSubagentSteeringReceipt {
                attempt_id: attempt_id.clone(),
                agent_id,
                steering_id,
                status: "queued".to_string(),
            });
        }
        let mut queue = pending.lock().await;
        if queue.len() >= self.limits.max_steering_messages {
            self.with_state(|state| {
                if let Some(active) = state.active.get_mut(attempt_id) {
                    active.steering_ids.remove(&steering_id);
                }
            });
            return Err(BackgroundSubagentError::SteeringQueueFull);
        }
        let mut bus = BusMessage::text(message, "main")
            .with_id(steering_id.clone())
            .with_target(agent_id.as_str());
        bus.metadata.insert(
            "starweaver.topic".to_string(),
            serde_json::json!("steering"),
        );
        queue.push_back(bus);
        drop(queue);
        if !self.with_state(|state| state.active.contains_key(attempt_id)) {
            return Err(BackgroundSubagentError::Terminal);
        }
        Ok(BackgroundSubagentSteeringReceipt {
            attempt_id: attempt_id.clone(),
            agent_id,
            steering_id,
            status: "queued".to_string(),
        })
    }

    /// Request idempotent cooperative cancellation of one active attempt.
    ///
    /// # Errors
    ///
    /// Returns an ownership or lifecycle error when the attempt is unknown.
    pub fn request_cancellation(
        &self,
        attempt_id: &SubagentAttemptId,
        cancellation_id: String,
    ) -> Result<BackgroundSubagentCancellationReceipt, BackgroundSubagentError> {
        self.request_cancellation_with_reason(attempt_id, cancellation_id, None)
    }

    /// Request cancellation and retain one bounded owner-supplied reason.
    ///
    /// # Errors
    ///
    /// Returns an ownership, lifecycle, or reason-size error when cancellation
    /// cannot be requested.
    pub fn request_cancellation_with_reason(
        &self,
        attempt_id: &SubagentAttemptId,
        cancellation_id: String,
        reason: Option<String>,
    ) -> Result<BackgroundSubagentCancellationReceipt, BackgroundSubagentError> {
        if cancellation_id.len() > self.limits.max_operation_id_bytes {
            return Err(BackgroundSubagentError::OperationIdTooLarge);
        }
        let reason = reason.filter(|value| !value.trim().is_empty());
        if reason
            .as_ref()
            .is_some_and(|value| value.len() > self.limits.max_cancellation_reason_bytes)
        {
            return Err(BackgroundSubagentError::CancellationReasonTooLarge);
        }
        if let Some(result) = self.task_result(attempt_id) {
            return Ok(BackgroundSubagentCancellationReceipt {
                attempt_id: attempt_id.clone(),
                agent_id: result.agent_id,
                cancellation_id,
                status: result.status.as_str().to_string(),
            });
        }
        let (agent_id, cancellation, abort_handle, start_timer) = self.with_state(|state| {
            let active = state
                .active
                .get_mut(attempt_id)
                .ok_or(BackgroundSubagentError::NotFound)?;
            let duplicate = active.cancellation_ids.contains(&cancellation_id);
            if !duplicate
                && active.cancellation_ids.len() >= self.limits.max_operation_ids_per_attempt
            {
                return Err(BackgroundSubagentError::OperationHistoryFull);
            }
            active.cancellation_ids.insert(cancellation_id.clone());
            if !duplicate && active.cancellation_reason.is_none() {
                active.cancellation_reason = reason;
            }
            let start_timer = !active.cancellation_timer_started;
            active.cancellation_timer_started = true;
            Ok::<_, BackgroundSubagentError>((
                active.info.agent_id.clone(),
                active.control.cancellation.clone(),
                active.abort_handle.clone(),
                start_timer,
            ))
        })?;
        cancellation.cancel();
        if start_timer && let Some(abort_handle) = abort_handle {
            let grace = self.limits.cancellation_grace;
            tokio::spawn(async move {
                tokio::time::sleep(grace).await;
                abort_handle.abort();
            });
        }
        Ok(BackgroundSubagentCancellationReceipt {
            attempt_id: attempt_id.clone(),
            agent_id,
            cancellation_id,
            status: "cancellation_requested".to_string(),
        })
    }

    pub(crate) fn begin_wait(&self, attempt_id: &SubagentAttemptId) {
        self.with_state(|state| {
            state.waiting_attempts.insert(attempt_id.clone());
        });
    }

    pub(crate) fn end_wait(&self, attempt_id: &SubagentAttemptId) {
        self.with_state(|state| {
            state.waiting_attempts.remove(attempt_id);
        });
    }

    pub(crate) fn is_waiting(&self, attempt_id: &SubagentAttemptId) -> bool {
        self.with_state(|state| state.waiting_attempts.contains(attempt_id))
    }

    /// Return the stable bus message id for one attempt result.
    #[must_use]
    pub fn get_task_result_message_id(&self, attempt_id: &SubagentAttemptId) -> String {
        format!("background-subagent-result:{}", attempt_id.as_str())
    }

    /// Atomically claim an undelivered result.
    ///
    /// # Errors
    ///
    /// Returns an ownership or delivery-state conflict when the result cannot
    /// be claimed.
    pub fn claim_delivery(
        &self,
        attempt_id: &SubagentAttemptId,
        claim: BackgroundSubagentDeliveryClaim,
    ) -> Result<BackgroundSubagentTaskResult, BackgroundSubagentError> {
        self.with_state(|state| {
            let result = state
                .results
                .get_mut(attempt_id)
                .ok_or(BackgroundSubagentError::NotFound)?;
            match result.delivery_status {
                BackgroundSubagentDeliveryStatus::Undelivered => {
                    result.delivery_status = BackgroundSubagentDeliveryStatus::Claimed;
                    result.delivery_claim = Some(claim);
                    Ok(result.clone())
                }
                BackgroundSubagentDeliveryStatus::Claimed => {
                    if result
                        .delivery_claim
                        .as_ref()
                        .is_some_and(|current| current.deadline <= Utc::now())
                    {
                        result.delivery_claim = Some(claim);
                        Ok(result.clone())
                    } else {
                        Err(BackgroundSubagentError::DeliveryClaimed)
                    }
                }
                BackgroundSubagentDeliveryStatus::Delivered => {
                    Err(BackgroundSubagentError::Delivered)
                }
            }
        })
    }

    /// Acknowledge one previously claimed logical delivery.
    ///
    /// # Errors
    ///
    /// Returns an ownership or claim conflict when `claim_id` does not own the
    /// current delivery claim.
    pub fn acknowledge_delivery(
        &self,
        attempt_id: &SubagentAttemptId,
        claim_id: &str,
    ) -> Result<(), BackgroundSubagentError> {
        self.with_state(|state| {
            let result = state
                .results
                .get_mut(attempt_id)
                .ok_or(BackgroundSubagentError::NotFound)?;
            if result.delivery_status == BackgroundSubagentDeliveryStatus::Delivered {
                return if result.delivered_claim_id.as_deref() == Some(claim_id) {
                    Ok(())
                } else {
                    Err(BackgroundSubagentError::Delivered)
                };
            }
            if result
                .delivery_claim
                .as_ref()
                .is_none_or(|claim| claim.claim_id != claim_id)
            {
                return Err(BackgroundSubagentError::DeliveryClaimed);
            }
            result.delivery_status = BackgroundSubagentDeliveryStatus::Delivered;
            result.delivery_claim = None;
            result.delivered_claim_id = Some(claim_id.to_string());
            let message_id = self.get_task_result_message_id(attempt_id);
            state
                .pending_messages
                .retain(|pending| pending.message.id != message_id);
            trim_results(state, self.limits.max_retained_results);
            Ok(())
        })
    }

    /// Release a failed, matching, unexpired delivery claim for retry.
    ///
    /// # Errors
    ///
    /// Returns an ownership or claim conflict when `claim_id` does not own the
    /// current delivery claim or the result was already delivered.
    pub fn release_delivery_claim(
        &self,
        attempt_id: &SubagentAttemptId,
        claim_id: &str,
    ) -> Result<(), BackgroundSubagentError> {
        self.with_state(|state| {
            let result = state
                .results
                .get_mut(attempt_id)
                .ok_or(BackgroundSubagentError::NotFound)?;
            if result.delivery_status == BackgroundSubagentDeliveryStatus::Delivered {
                return Err(BackgroundSubagentError::Delivered);
            }
            if result.delivery_status != BackgroundSubagentDeliveryStatus::Claimed {
                return Err(BackgroundSubagentError::DeliveryClaimed);
            }
            if result
                .delivery_claim
                .as_ref()
                .is_none_or(|claim| claim.claim_id != claim_id)
            {
                return Err(BackgroundSubagentError::DeliveryClaimed);
            }
            result.delivery_status = BackgroundSubagentDeliveryStatus::Undelivered;
            result.delivery_claim = None;
            Ok(())
        })?;
        self.notify.notify_waiters();
        self.notify_completion(attempt_id);
        Ok(())
    }

    pub(crate) fn enqueue_message(&self, attempt_id: SubagentAttemptId, message: BusMessage) {
        self.with_state(|state| {
            if state
                .pending_messages
                .iter()
                .any(|pending| pending.attempt_id == attempt_id || pending.message.id == message.id)
            {
                return;
            }
            state.pending_messages.push_back(PendingResultMessage {
                attempt_id,
                message,
            });
        });
        self.notify.notify_waiters();
    }

    /// Return whether undelivered completion messages await a parent turn.
    #[must_use]
    pub fn has_pending_messages(&self) -> bool {
        self.with_state(|state| !state.pending_messages.is_empty())
    }

    pub(super) fn claim_pending_messages(
        &self,
        claim_scope: &str,
        continuation_run_id: Option<&RunId>,
    ) -> Vec<ClaimedBackgroundSubagentMessage> {
        self.with_state(|state| {
            let now = Utc::now();
            let mut claimed = Vec::new();
            for pending in &state.pending_messages {
                let Some(result) = state.results.get_mut(&pending.attempt_id) else {
                    continue;
                };
                let owned_claim = result.delivery_claim.as_ref().filter(|current| {
                    result.delivery_status == BackgroundSubagentDeliveryStatus::Claimed
                        && continuation_run_id.is_some()
                        && current.continuation_run_id.as_ref() == continuation_run_id
                });
                if let Some(current) = owned_claim {
                    claimed.push(ClaimedBackgroundSubagentMessage {
                        attempt_id: pending.attempt_id.clone(),
                        claim_id: current.claim_id.clone(),
                        message: pending.message.clone(),
                    });
                    continue;
                }
                let claimable = result.delivery_status
                    == BackgroundSubagentDeliveryStatus::Undelivered
                    || (result.delivery_status == BackgroundSubagentDeliveryStatus::Claimed
                        && result
                            .delivery_claim
                            .as_ref()
                            .is_some_and(|current| current.deadline <= now));
                if !claimable {
                    continue;
                }
                let claim_id = format!(
                    "bus:{claim_scope}:{}:{}",
                    pending.attempt_id.as_str(),
                    uuid::Uuid::new_v4()
                );
                result.delivery_status = BackgroundSubagentDeliveryStatus::Claimed;
                result.delivery_claim = Some(BackgroundSubagentDeliveryClaim {
                    claim_id: claim_id.clone(),
                    continuation_run_id: continuation_run_id.cloned(),
                    deadline: now + chrono::Duration::seconds(60),
                });
                claimed.push(ClaimedBackgroundSubagentMessage {
                    attempt_id: pending.attempt_id.clone(),
                    claim_id,
                    message: pending.message.clone(),
                });
            }
            claimed
        })
    }

    /// Wait until one attempt becomes terminal, using one absolute timeout.
    pub async fn wait_for_attempt(
        &self,
        attempt_id: &SubagentAttemptId,
        timeout: Duration,
    ) -> Option<BackgroundSubagentTaskResult> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let notified = self.notify.notified();
            if let Some(result) = self.task_result(attempt_id) {
                return Some(result);
            }
            if !self
                .active_tasks()
                .iter()
                .any(|info| &info.attempt_id == attempt_id)
                || timeout.is_zero()
            {
                return None;
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return None;
            }
        }
    }

    /// Wait once for the supplied known attempt set.
    pub async fn wait_for_attempts(
        &self,
        attempt_ids: &[SubagentAttemptId],
        timeout: Duration,
    ) -> BTreeMap<SubagentAttemptId, Option<BackgroundSubagentTaskResult>> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let notified = self.notify.notified();
            let ready = attempt_ids.iter().all(|attempt_id| {
                self.task_result(attempt_id).is_some()
                    || !self
                        .active_tasks()
                        .iter()
                        .any(|info| &info.attempt_id == attempt_id)
            });
            if ready || timeout.is_zero() {
                break;
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                break;
            }
        }
        attempt_ids
            .iter()
            .map(|attempt_id| (attempt_id.clone(), self.task_result(attempt_id)))
            .collect()
    }

    /// Stop admission, cooperatively cancel active children, then abort after one deadline.
    pub async fn shutdown(&self, timeout: Option<Duration>) {
        let timeout = timeout.unwrap_or(self.limits.shutdown_grace);
        let (controls, abort_handles) = self.with_state(|state| {
            state.closing = true;
            let controls = state
                .active
                .values()
                .map(|active| active.control.cancellation.clone())
                .collect::<Vec<_>>();
            let abort_handles = state
                .active
                .values()
                .filter_map(|active| active.abort_handle.clone())
                .collect::<Vec<_>>();
            (controls, abort_handles)
        });
        for control in controls {
            control.cancel();
        }
        let started_at = tokio::time::Instant::now();
        let final_deadline = started_at + timeout;
        let cooperative_deadline = started_at + timeout.mul_f32(0.8);
        loop {
            let notified = self.notify.notified();
            if !self.has_active_tasks() {
                return;
            }
            if timeout.is_zero()
                || tokio::time::timeout_at(cooperative_deadline, notified)
                    .await
                    .is_err()
            {
                break;
            }
        }
        // Re-read handles before forced abort so an attempt accepted immediately
        // before shutdown cannot escape merely because its worker attached after
        // the initial snapshot.
        let late_abort_handles = self.with_state(|state| {
            state
                .active
                .values()
                .filter_map(|active| active.abort_handle.clone())
                .collect::<Vec<_>>()
        });
        for abort_handle in abort_handles.into_iter().chain(late_abort_handles) {
            abort_handle.abort();
        }
        let finalizers = self.with_state(|state| std::mem::take(&mut state.finalizer_handles));
        for (_, finalizer) in finalizers {
            let _ = tokio::time::timeout_at(final_deadline, finalizer).await;
        }
        // The outer finalizer records terminal evidence after the owned worker is
        // aborted. Reserve the remainder of the same absolute shutdown deadline
        // for that finalizer rather than returning with detached active state.
        while self.has_active_tasks() && tokio::time::Instant::now() < final_deadline {
            let notified = self.notify.notified();
            if tokio::time::timeout_at(final_deadline, notified)
                .await
                .is_err()
            {
                break;
            }
        }
        self.force_interrupt_active();
        let late_finalizers = self.with_state(|state| std::mem::take(&mut state.finalizer_handles));
        for (_, finalizer) in late_finalizers {
            finalizer.abort();
        }
    }

    fn force_interrupt_active(&self) {
        let attempt_ids = self.with_state(|state| state.active.keys().cloned().collect::<Vec<_>>());
        for attempt_id in attempt_ids {
            let Some(result) = self.record_terminal(
                &attempt_id,
                BackgroundSubagentExecutionStatus::Cancelled,
                None,
                Some("background subagent interrupted during host shutdown".to_string()),
            ) else {
                continue;
            };
            let message = BusMessage::text(
                format!(
                    "Background delegate '{}' (agent_id: {}, attempt_id: {}) was interrupted during host shutdown",
                    result.subagent_name,
                    result.agent_id,
                    result.attempt_id.as_str(),
                ),
                result.agent_id.clone(),
            )
            .with_id(self.get_task_result_message_id(&attempt_id))
            .with_target("main");
            self.enqueue_message(attempt_id.clone(), message);
            self.notify_completion(&attempt_id);
        }
    }
}

fn trim_results(state: &mut BackgroundSubagentState, max_results: usize) {
    while state.results.len() > max_results {
        let oldest = state
            .results
            .iter()
            .filter(|(attempt_id, result)| {
                result.delivery_status == BackgroundSubagentDeliveryStatus::Delivered
                    && !state.context_deltas.contains_key(*attempt_id)
                    && !state
                        .pending_messages
                        .iter()
                        .any(|pending| &pending.attempt_id == *attempt_id)
            })
            .min_by_key(|(_, result)| result.completed_at)
            .map(|(attempt_id, _)| attempt_id.clone());
        let Some(oldest) = oldest else {
            break;
        };
        state.results.remove(&oldest);
    }
}

fn bounded_text(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes.saturating_sub(3).min(value.len());
    while !value.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    format!("{}...", &value[..end])
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    fn accept(
        supervisor: &BackgroundSubagentSupervisor,
        attempt_id: &SubagentAttemptId,
        agent_id: &str,
        is_resume: bool,
    ) -> Result<BackgroundSubagentTaskInfo, BackgroundSubagentError> {
        supervisor.accept(BackgroundSubagentAcceptance {
            attempt_id: attempt_id.clone(),
            agent_id: agent_id.to_string(),
            subagent_name: "child".to_string(),
            linked_task_id: None,
            prompt: "bounded task".to_string(),
            parent_session_id: None,
            parent_run_id: None,
            is_resume,
        })
    }

    #[test]
    fn attempts_are_isolated_and_active_conversations_conflict() {
        let supervisor = BackgroundSubagentSupervisor::new();
        let first = SubagentAttemptId::from_string("subattempt_first");
        let second = SubagentAttemptId::from_string("subattempt_second");

        accept(&supervisor, &first, "child-bg-stable", false).unwrap();
        assert_eq!(
            accept(&supervisor, &second, "child-bg-stable", true).unwrap_err(),
            BackgroundSubagentError::ActiveConversation
        );
        supervisor.record_terminal(
            &first,
            BackgroundSubagentExecutionStatus::Completed,
            Some("first result".to_string()),
            None,
        );
        accept(&supervisor, &second, "child-bg-stable", true).unwrap();
        supervisor.record_terminal(
            &second,
            BackgroundSubagentExecutionStatus::Completed,
            Some("second result".to_string()),
            None,
        );

        assert_eq!(
            supervisor.task_result(&first).unwrap().content.as_deref(),
            Some("first result")
        );
        assert_eq!(
            supervisor.task_result(&second).unwrap().content.as_deref(),
            Some("second result")
        );
    }

    #[tokio::test]
    async fn steering_is_idempotent_and_bounded() {
        let limits = BackgroundSubagentLimits {
            max_steering_messages: 1,
            max_steering_bytes: 8,
            ..BackgroundSubagentLimits::default()
        };
        let supervisor = BackgroundSubagentSupervisor::with_limits(limits);
        let attempt = SubagentAttemptId::from_string("subattempt_steer");
        accept(&supervisor, &attempt, "child-bg-steer", false).unwrap();

        let first = supervisor
            .steer(&attempt, "hello".to_string(), "steer-1".to_string())
            .await
            .unwrap();
        let duplicate = supervisor
            .steer(&attempt, "hello".to_string(), "steer-1".to_string())
            .await
            .unwrap();
        assert_eq!(first, duplicate);
        assert_eq!(
            supervisor
                .child_control(&attempt)
                .unwrap()
                .pending_messages
                .lock()
                .await
                .len(),
            1
        );
        assert_eq!(
            supervisor
                .steer(&attempt, "second".to_string(), "steer-2".to_string())
                .await
                .unwrap_err(),
            BackgroundSubagentError::SteeringQueueFull
        );
        assert_eq!(
            supervisor
                .steer(&attempt, "too large".to_string(), "steer-3".to_string())
                .await
                .unwrap_err(),
            BackgroundSubagentError::SteeringTooLarge
        );
    }

    #[test]
    fn cancellation_is_idempotent_and_terminal_races_are_observable() {
        let supervisor = BackgroundSubagentSupervisor::new();
        let attempt = SubagentAttemptId::from_string("subattempt_cancel");
        accept(&supervisor, &attempt, "child-bg-cancel", false).unwrap();
        let control = supervisor.child_control(&attempt).unwrap();

        let first = supervisor
            .request_cancellation_with_reason(
                &attempt,
                "cancel-1".to_string(),
                Some("no longer needed".to_string()),
            )
            .unwrap();
        let duplicate = supervisor
            .request_cancellation_with_reason(
                &attempt,
                "cancel-1".to_string(),
                Some("ignored duplicate".to_string()),
            )
            .unwrap();
        assert_eq!(first, duplicate);
        assert!(control.cancellation.is_cancelled());

        supervisor.record_terminal(
            &attempt,
            BackgroundSubagentExecutionStatus::Cancelled,
            None,
            Some("cancelled".to_string()),
        );
        let terminal = supervisor
            .request_cancellation(&attempt, "cancel-2".to_string())
            .unwrap();
        assert_eq!(terminal.status, "cancelled");
        let result = supervisor.task_result(&attempt).unwrap();
        assert_eq!(result.failure_category.as_deref(), Some("cancelled"));
        assert_eq!(
            result.cancellation_reason.as_deref(),
            Some("no longer needed")
        );
    }

    #[test]
    fn late_context_delta_replays_once_into_a_restored_parent() {
        let supervisor = BackgroundSubagentSupervisor::new();
        let attempt = SubagentAttemptId::from_string("subattempt_delta");
        let parent_run = RunId::from_string("run_parent_delta");
        supervisor.begin_parent_run(parent_run.clone());
        supervisor
            .accept(BackgroundSubagentAcceptance {
                attempt_id: attempt.clone(),
                agent_id: "child-bg-delta".to_string(),
                subagent_name: "child".to_string(),
                linked_task_id: None,
                prompt: "task".to_string(),
                parent_session_id: None,
                parent_run_id: Some(parent_run.clone()),
                is_resume: false,
            })
            .unwrap();

        let handle = starweaver_context::AgentContextHandle::new(AgentContext::default());
        let mut source = AgentContext::default();
        source.usage.requests = 2;
        source.subagent_history.insert(
            "child-bg-delta".to_string(),
            vec![ModelMessage::Response(
                starweaver_model::ModelResponse::text("child result"),
            )],
        );
        source.publish_event(AgentEvent::new(
            "subagent_completed",
            serde_json::json!({"attempt_id": attempt.as_str()}),
        ));
        let delta = BackgroundSubagentContextDelta::from_context(
            &source,
            &Usage::default(),
            &BTreeSet::new(),
            0,
            "child-bg-delta",
        );
        supervisor.end_parent_run(&parent_run);
        supervisor.merge_or_stage_context_delta(&attempt, &handle, delta);
        assert_eq!(handle.snapshot().usage.requests, 0);

        let mut restored = AgentContext::default();
        supervisor.apply_context_deltas(&mut restored);
        supervisor.apply_context_deltas(&mut restored);
        assert_eq!(restored.usage.requests, 2);
        assert_eq!(restored.subagent_history["child-bg-delta"].len(), 1);
        assert_eq!(
            restored
                .events
                .events()
                .iter()
                .filter(|event| event.kind == "subagent_completed")
                .count(),
            1
        );
    }

    #[test]
    fn delivery_claims_are_atomic_and_completion_callback_is_single_source() {
        let completions = Arc::new(AtomicUsize::new(0));
        let callback_count = completions.clone();
        let supervisor = BackgroundSubagentSupervisor::new().with_completion_callback(Arc::new(
            move |_result: &BackgroundSubagentTaskResult| {
                callback_count.fetch_add(1, Ordering::SeqCst);
            },
        ));
        let attempt = SubagentAttemptId::from_string("subattempt_delivery");
        accept(&supervisor, &attempt, "child-bg-delivery", false).unwrap();
        supervisor.record_terminal(
            &attempt,
            BackgroundSubagentExecutionStatus::Completed,
            Some("done".to_string()),
            None,
        );
        supervisor.notify_completion(&attempt);
        assert_eq!(completions.load(Ordering::SeqCst), 1);

        let claim = BackgroundSubagentDeliveryClaim {
            claim_id: "claim-1".to_string(),
            continuation_run_id: None,
            deadline: Utc::now() + chrono::Duration::seconds(30),
        };
        supervisor.claim_delivery(&attempt, claim).unwrap();
        let conflicting = BackgroundSubagentDeliveryClaim {
            claim_id: "claim-2".to_string(),
            continuation_run_id: None,
            deadline: Utc::now() + chrono::Duration::seconds(30),
        };
        assert_eq!(
            supervisor
                .claim_delivery(&attempt, conflicting)
                .unwrap_err(),
            BackgroundSubagentError::DeliveryClaimed
        );
        supervisor
            .acknowledge_delivery(&attempt, "claim-1")
            .unwrap();
        assert_eq!(
            supervisor.task_result(&attempt).unwrap().delivery_status,
            BackgroundSubagentDeliveryStatus::Delivered
        );
        supervisor.notify_completion(&attempt);
        assert_eq!(completions.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn pending_delivery_never_steals_an_unexpired_claim() {
        let supervisor = BackgroundSubagentSupervisor::new();
        let attempt = SubagentAttemptId::from_string("subattempt_claimed_pending");
        accept(&supervisor, &attempt, "child-bg-claimed", false).unwrap();
        supervisor.record_terminal(
            &attempt,
            BackgroundSubagentExecutionStatus::Completed,
            Some("done".to_string()),
            None,
        );
        supervisor.enqueue_message(
            attempt.clone(),
            BusMessage::text("done", "child-bg-claimed")
                .with_id(supervisor.get_task_result_message_id(&attempt))
                .with_target("main"),
        );
        supervisor
            .claim_delivery(
                &attempt,
                BackgroundSubagentDeliveryClaim {
                    claim_id: "external-claim".to_string(),
                    continuation_run_id: Some(RunId::from_string("run_external")),
                    deadline: Utc::now() + chrono::Duration::seconds(30),
                },
            )
            .unwrap();

        assert!(
            supervisor
                .claim_pending_messages("run_other", Some(&RunId::from_string("run_other")))
                .is_empty()
        );
        let result = supervisor.task_result(&attempt).unwrap();
        assert_eq!(
            result
                .delivery_claim
                .as_ref()
                .map(|claim| claim.claim_id.as_str()),
            Some("external-claim")
        );
        assert_eq!(
            supervisor
                .acknowledge_delivery(&attempt, "wrong-claim")
                .unwrap_err(),
            BackgroundSubagentError::DeliveryClaimed
        );
        supervisor
            .acknowledge_delivery(&attempt, "external-claim")
            .unwrap();
        assert_eq!(
            supervisor
                .acknowledge_delivery(&attempt, "wrong-claim")
                .unwrap_err(),
            BackgroundSubagentError::Delivered
        );
    }

    #[test]
    fn undelivered_retention_applies_admission_backpressure() {
        let supervisor = BackgroundSubagentSupervisor::with_limits(BackgroundSubagentLimits {
            max_retained_results: 1,
            ..BackgroundSubagentLimits::default()
        });
        let first = SubagentAttemptId::from_string("subattempt_retained_first");
        accept(&supervisor, &first, "child-bg-retained-first", false).unwrap();
        supervisor.record_terminal(
            &first,
            BackgroundSubagentExecutionStatus::Completed,
            Some("must remain available".to_string()),
            None,
        );
        let second = SubagentAttemptId::from_string("subattempt_retained_second");
        assert_eq!(
            accept(&supervisor, &second, "child-bg-retained-second", false).unwrap_err(),
            BackgroundSubagentError::QuotaExceeded
        );
        assert_eq!(
            supervisor.task_result(&first).unwrap().content.as_deref(),
            Some("must remain available")
        );
    }

    #[tokio::test]
    async fn shutdown_terminalizes_attempts_without_attached_workers() {
        let supervisor = BackgroundSubagentSupervisor::new();
        let attempt = SubagentAttemptId::from_string("subattempt_shutdown_forced");
        accept(&supervisor, &attempt, "child-bg-shutdown", false).unwrap();

        supervisor.shutdown(Some(Duration::ZERO)).await;

        assert!(!supervisor.has_active_tasks());
        let result = supervisor.task_result(&attempt).unwrap();
        assert_eq!(result.status, BackgroundSubagentExecutionStatus::Cancelled);
        assert_eq!(
            result.delivery_status,
            BackgroundSubagentDeliveryStatus::Undelivered
        );
        assert!(supervisor.has_pending_messages());
    }

    #[tokio::test]
    async fn wait_uses_one_absolute_deadline_and_shutdown_closes_admission() {
        let supervisor = Arc::new(BackgroundSubagentSupervisor::new());
        let attempt = SubagentAttemptId::from_string("subattempt_wait");
        accept(&supervisor, &attempt, "child-bg-wait", false).unwrap();
        let completing = supervisor.clone();
        let completing_attempt = attempt.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            completing.record_terminal(
                &completing_attempt,
                BackgroundSubagentExecutionStatus::Completed,
                Some("done".to_string()),
                None,
            );
        });
        assert_eq!(
            supervisor
                .wait_for_attempt(&attempt, Duration::from_secs(1))
                .await
                .unwrap()
                .content
                .as_deref(),
            Some("done")
        );

        supervisor.shutdown(Some(Duration::ZERO)).await;
        let rejected = SubagentAttemptId::from_string("subattempt_rejected");
        assert_eq!(
            accept(&supervisor, &rejected, "child-bg-rejected", false).unwrap_err(),
            BackgroundSubagentError::Closing
        );
    }
}
