//! Host-owned lifecycle supervisor for asynchronous subagent attempts.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    future::Future,
    sync::{Arc, Mutex, Weak},
    time::Duration,
};

use chrono::{DateTime, Utc};
use futures_util::FutureExt as _;
use serde::{Deserialize, Serialize};
use starweaver_context::{AgentContext, AgentEvent, AgentInfo, BusMessage};
use starweaver_core::{
    CancellationToken, RunId, SessionId, SubagentAttemptId, TaskId, TraceContext,
};
use starweaver_model::ModelMessage;
use starweaver_session::{
    BACKGROUND_SUBAGENT_RECORD_VERSION, BackgroundSubagentArtifact,
    BackgroundSubagentArtifactLimits, BackgroundSubagentRecord, BackgroundSubagentTerminalCommit,
    DurableBackgroundSubagentDeliveryClaim, DurableBackgroundSubagentDeliveryRelease,
    DurableBackgroundSubagentDeliveryStatus, DurableBackgroundSubagentExecutionStatus,
    DurableBackgroundSubagentOwnerLease, DurableBackgroundSubagentResultRef,
    DurableBackgroundSubagentRetentionStatus, SessionStore, SessionStoreError,
    background_subagent_result_digest,
};
use starweaver_usage::{Usage, UsageSnapshotEntry};
use tokio::task::JoinHandle;

const DEFAULT_MAX_ACTIVE_ATTEMPTS: usize = 8;
const DEFAULT_MAX_RETAINED_RESULTS: usize = 128;
const DEFAULT_MAX_STEERING_MESSAGES: usize = 32;
const DEFAULT_MAX_STEERING_BYTES: usize = 16 * 1024;
const DEFAULT_MAX_PROMPT_BYTES: usize = 64 * 1024;
const DEFAULT_MAX_CANCELLATION_REASON_BYTES: usize = 1024;
const DEFAULT_MAX_RESULT_BYTES: usize = 64 * 1024;
const DEFAULT_MAX_ARTIFACT_RESULT_BYTES: usize = 8 * 1024 * 1024;
const DEFAULT_MAX_RETAINED_ARTIFACT_BYTES: usize = 64 * 1024 * 1024;
const DEFAULT_MAX_OPERATION_ID_BYTES: usize = 256;
const DEFAULT_MAX_OPERATION_IDS: usize = 256;
const DEFAULT_SHUTDOWN_GRACE: Duration = Duration::from_secs(5);
const DEFAULT_DURABLE_OWNER_LEASE_TTL: Duration = Duration::from_secs(30);
const DEFAULT_RESULT_RETENTION: Duration = Duration::from_hours(24);

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
    /// Maximum bytes accepted for one externalized successful result artifact.
    pub max_artifact_result_bytes: usize,
    /// Maximum aggregate artifact bytes retained by this supervisor scope.
    pub max_retained_artifact_bytes: usize,
    /// Maximum UTF-8 bytes accepted in an idempotency operation id.
    pub max_operation_id_bytes: usize,
    /// Maximum steering/cancellation operation ids retained per attempt.
    pub max_operation_ids_per_attempt: usize,
    /// Cooperative cancellation grace period before task abort.
    pub cancellation_grace: Duration,
    /// Grace period used by default shutdown.
    pub shutdown_grace: Duration,
    /// Policy-controlled lifetime for retained inline previews and artifacts.
    pub retained_result_ttl: Duration,
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
            max_artifact_result_bytes: DEFAULT_MAX_ARTIFACT_RESULT_BYTES,
            max_retained_artifact_bytes: DEFAULT_MAX_RETAINED_ARTIFACT_BYTES,
            max_operation_id_bytes: DEFAULT_MAX_OPERATION_ID_BYTES,
            max_operation_ids_per_attempt: DEFAULT_MAX_OPERATION_IDS,
            cancellation_grace: DEFAULT_SHUTDOWN_GRACE,
            shutdown_grace: DEFAULT_SHUTDOWN_GRACE,
            retained_result_ttl: DEFAULT_RESULT_RETENTION,
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
    /// Host-owned reference for oversized successful output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_ref: Option<String>,
    /// Policy deadline for retained content.
    pub retention_expires_at: DateTime<Utc>,
    /// Logical result size before inline preview truncation.
    pub logical_size_bytes: u64,
    /// SHA-256 digest of the logical result before inline preview truncation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    /// Completion timestamp.
    pub completed_at: DateTime<Utc>,
}

impl BackgroundSubagentTaskResult {
    pub(crate) fn terminal(
        info: &BackgroundSubagentTaskInfo,
        status: BackgroundSubagentExecutionStatus,
        content: Option<&str>,
        error: Option<&str>,
        max_inline_bytes: usize,
        retention_ttl: Duration,
    ) -> Self {
        debug_assert!(status.is_terminal());
        let logical_size = content
            .map_or(0, str::len)
            .saturating_add(error.map_or(0, str::len));
        let digest = (logical_size > 0).then(|| background_subagent_result_digest(content, error));
        let content = content.map(|value| bounded_text(value, max_inline_bytes));
        let error = error.map(|value| bounded_text(value, max_inline_bytes));
        let completed_at = Utc::now();
        let retention_expires_at = completed_at
            + chrono::Duration::from_std(retention_ttl)
                .unwrap_or_else(|_| chrono::Duration::seconds(86_400));
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
            artifact_ref: None,
            retention_expires_at,
            logical_size_bytes: u64::try_from(logical_size).unwrap_or(u64::MAX),
            digest,
            completed_at,
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
    /// An operation id was reused with a different payload.
    #[error("background subagent operation id was reused with different input")]
    IdempotencyConflict,
    /// Delivery is already claimed by another consumer.
    #[error("background subagent result delivery is already claimed")]
    DeliveryClaimed,
    /// Result was already logically delivered.
    #[error("background subagent result was already delivered")]
    Delivered,
    /// A required durable lifecycle or delivery operation failed.
    #[error("background subagent durability operation failed: {0}")]
    Durability(String),
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DurableLeaseRefresh {
    Renewed,
    RetryableFailure,
    TerminalObserved,
    ConfirmedOwnerLoss,
}

enum DurableTerminalPersistenceError {
    Retryable(BackgroundSubagentError),
    ConfirmedOwnerLoss,
}

#[derive(Clone)]
struct PendingTerminalEvidence {
    result: BackgroundSubagentTaskResult,
    artifact: Option<BackgroundSubagentArtifact>,
}

#[derive(Clone)]
struct ActiveAttempt {
    info: BackgroundSubagentTaskInfo,
    control: BackgroundSubagentChildControl,
    steering_ids: BTreeMap<String, String>,
    cancellation_ids: BTreeMap<String, Option<String>>,
    cancellation_reason: Option<String>,
    pending_terminal: Option<PendingTerminalEvidence>,
}

#[derive(Clone)]
pub(super) struct BackgroundSubagentContextDelta {
    usage: Usage,
    usage_snapshot_entries: BTreeMap<String, UsageSnapshotEntry>,
    agent_registry: BTreeMap<String, AgentInfo>,
    subagent_history: BTreeMap<String, (Vec<ModelMessage>, Vec<ModelMessage>)>,
    events: Vec<AgentEvent>,
}

impl BackgroundSubagentContextDelta {
    pub(crate) fn from_context(
        source: &AgentContext,
        base_usage: &Usage,
        base_usage_snapshot_keys: &BTreeSet<String>,
        base_event_count: usize,
        agent_id: &str,
        base_subagent_history: &[ModelMessage],
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
                .and_then(|history| {
                    history
                        .as_slice()
                        .strip_prefix(base_subagent_history)
                        .map(|appended| {
                            BTreeMap::from([(
                                agent_id.to_string(),
                                (base_subagent_history.to_vec(), appended.to_vec()),
                            )])
                        })
                })
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
        for (agent_id, (base, appended)) in &self.subagent_history {
            let history = target.subagent_history.entry(agent_id.clone()).or_default();
            if history.as_slice() == base.as_slice() {
                history.extend(appended.iter().cloned());
            }
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

struct FinalizerCompletionGuard {
    supervisor: Weak<BackgroundSubagentSupervisor>,
    attempt_id: SubagentAttemptId,
}

impl Drop for FinalizerCompletionGuard {
    fn drop(&mut self) {
        if let Some(supervisor) = self.supervisor.upgrade() {
            supervisor.finish_finalizer(&self.attempt_id);
        }
    }
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
    completed_finalizers: BTreeSet<SubagentAttemptId>,
}

/// Host-owned supervisor for asynchronous subagent attempts.
///
/// The supervisor is intentionally injectable and may outlive individual parent
/// runtimes. It owns cancellation/abort handles, attempt-keyed terminal evidence,
/// delivery claims, bounded retention, and pending parent messages.
pub struct BackgroundSubagentSupervisor {
    state: Mutex<BackgroundSubagentState>,
    shutdown_gate: tokio::sync::Mutex<()>,
    notify: tokio::sync::Notify,
    limits: BackgroundSubagentLimits,
    completion_callback: Option<Arc<dyn BackgroundSubagentCompletionCallback>>,
    durable_store: Option<Arc<dyn SessionStore>>,
    durable_namespace: String,
    durable_host_instance_id: String,
    durable_fencing_generation: u64,
    durable_owner_lease_ttl: Duration,
    #[cfg(test)]
    terminal_commit_failures_remaining: std::sync::atomic::AtomicUsize,
    #[cfg(test)]
    heartbeat_failures_remaining: std::sync::atomic::AtomicUsize,
    #[cfg(test)]
    terminal_commit_response_failures_remaining: std::sync::atomic::AtomicUsize,
    #[cfg(test)]
    terminal_persistence_delay_millis: std::sync::atomic::AtomicUsize,
}

struct FinalizerDrainGuard<'a> {
    supervisor: &'a BackgroundSubagentSupervisor,
    handles: BTreeMap<SubagentAttemptId, JoinHandle<()>>,
}

impl Drop for FinalizerDrainGuard<'_> {
    fn drop(&mut self) {
        for (attempt_id, handle) in std::mem::take(&mut self.handles) {
            let mut handle = Some(handle);
            self.supervisor.with_state(|state| {
                if !state.completed_finalizers.remove(&attempt_id)
                    && !state.finalizer_handles.contains_key(&attempt_id)
                    && let Some(handle) = handle.take()
                {
                    state.finalizer_handles.insert(attempt_id, handle);
                }
            });
            if let Some(handle) = handle {
                handle.abort();
            }
        }
        self.supervisor.notify.notify_waiters();
    }
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
            shutdown_gate: tokio::sync::Mutex::new(()),
            notify: tokio::sync::Notify::new(),
            limits,
            completion_callback: None,
            durable_store: None,
            durable_namespace: starweaver_session::LOCAL_SESSION_NAMESPACE.to_string(),
            durable_host_instance_id: format!("sdk-host-{}", uuid::Uuid::new_v4()),
            durable_fencing_generation: 1,
            durable_owner_lease_ttl: DEFAULT_DURABLE_OWNER_LEASE_TTL,
            #[cfg(test)]
            terminal_commit_failures_remaining: std::sync::atomic::AtomicUsize::new(0),
            #[cfg(test)]
            heartbeat_failures_remaining: std::sync::atomic::AtomicUsize::new(0),
            #[cfg(test)]
            terminal_commit_response_failures_remaining: std::sync::atomic::AtomicUsize::new(0),
            #[cfg(test)]
            terminal_persistence_delay_millis: std::sync::atomic::AtomicUsize::new(0),
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

    /// Attach a durable session store used before acceptance, terminal visibility, and delivery CAS.
    #[must_use]
    pub fn with_durable_store(
        mut self,
        store: Arc<dyn SessionStore>,
        namespace: impl Into<String>,
    ) -> Self {
        self.durable_store = Some(store);
        self.durable_namespace = namespace.into();
        self
    }

    /// Configure the fenced service-host owner used for durable execution writes.
    #[must_use]
    pub fn with_durable_owner(
        mut self,
        host_instance_id: impl Into<String>,
        fencing_generation: u64,
        lease_ttl: Duration,
    ) -> Self {
        self.durable_host_instance_id = host_instance_id.into();
        self.durable_fencing_generation = fencing_generation;
        self.durable_owner_lease_ttl = lease_ttl;
        self
    }

    /// Return whether delivery requires durable parent-commit acknowledgement.
    #[must_use]
    pub fn durable_delivery_enabled(&self) -> bool {
        self.durable_store.is_some()
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

    pub(super) fn publish_committed_context_delta(
        &self,
        attempt_id: &SubagentAttemptId,
        context_handle: &starweaver_context::AgentContextHandle,
        delta: &BackgroundSubagentContextDelta,
    ) {
        let published = self.with_state(|state| {
            if !state.results.contains_key(attempt_id) {
                return false;
            }
            state
                .context_deltas
                .insert(attempt_id.clone(), delta.clone());
            true
        });
        if published && self.parent_run_is_active(attempt_id) {
            context_handle.update(|context| delta.apply(attempt_id, context));
        }
    }

    pub(crate) fn apply_context_deltas(&self, context: &mut AgentContext) {
        let deltas = self.with_state(|state| {
            let pending = std::mem::take(&mut state.context_deltas);
            let mut committed = BTreeMap::new();
            for (attempt_id, delta) in pending {
                if state.results.contains_key(&attempt_id) {
                    committed.insert(attempt_id, delta);
                } else {
                    state.context_deltas.insert(attempt_id, delta);
                }
            }
            committed
        });
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
                    steering_ids: BTreeMap::new(),
                    cancellation_ids: BTreeMap::new(),
                    cancellation_reason: None,
                    pending_terminal: None,
                },
            );
            Ok(info.clone())
        })?;
        self.notify.notify_waiters();
        Ok(info)
    }

    /// Reserve an attempt and persist durable acceptance before exposing its identity.
    pub(super) async fn accept_durable(
        &self,
        acceptance: BackgroundSubagentAcceptance,
    ) -> Result<BackgroundSubagentTaskInfo, BackgroundSubagentError> {
        let info = self.accept(acceptance)?;
        let Some(store) = self.durable_store.as_ref() else {
            return Ok(info);
        };
        let persisted = async {
            let parent_session_id = info.parent_session_id.as_ref().ok_or_else(|| {
                BackgroundSubagentError::Durability(
                    "durable supervisor requires parent_session_id".to_string(),
                )
            })?;
            let parent_run_id = info.parent_run_id.as_ref().ok_or_else(|| {
                BackgroundSubagentError::Durability(
                    "durable supervisor requires parent_run_id".to_string(),
                )
            })?;
            let session = store
                .load_session(parent_session_id)
                .await
                .map_err(|error| BackgroundSubagentError::Durability(error.to_string()))?;
            let run = store
                .load_run(parent_session_id, parent_run_id)
                .await
                .map_err(|error| BackgroundSubagentError::Durability(error.to_string()))?;
            let profile = run.profile.clone().or(session.profile).ok_or_else(|| {
                BackgroundSubagentError::Durability(
                    "durable parent has no resolved profile".to_string(),
                )
            })?;
            let record = durable_acceptance_record(
                &info,
                &self.durable_namespace,
                profile,
                run.trace_context,
                &self.durable_host_instance_id,
                self.durable_fencing_generation,
                self.durable_owner_lease_ttl,
            )?;
            store
                .record_background_subagent_acceptance(record)
                .await
                .map_err(|error| match error {
                    SessionStoreError::Conflict(message)
                        if message.contains("already has an active durable attempt") =>
                    {
                        BackgroundSubagentError::ActiveConversation
                    }
                    error => BackgroundSubagentError::Durability(error.to_string()),
                })?;
            Ok::<(), BackgroundSubagentError>(())
        }
        .await;
        if let Err(error) = persisted {
            self.rollback_acceptance(&info);
            return Err(error);
        }
        Ok(info)
    }

    fn rollback_acceptance(&self, info: &BackgroundSubagentTaskInfo) {
        self.with_state(|state| {
            state.active.remove(&info.attempt_id);
            state.active_by_agent.remove(&info.agent_id);
            if !info.is_resume {
                state.conversations.remove(&info.agent_id);
            }
        });
        self.notify.notify_waiters();
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

    pub(super) fn spawn_finalizer<F>(
        self: &Arc<Self>,
        attempt_id: SubagentAttemptId,
        future: F,
    ) -> Result<(), BackgroundSubagentError>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let mut future = Some(future);
        self.with_state(|state| {
            if state.closing {
                return Err(BackgroundSubagentError::Closing);
            }
            if !state.active.contains_key(&attempt_id) {
                return Err(BackgroundSubagentError::NotFound);
            }
            if state.finalizer_handles.contains_key(&attempt_id) {
                return Err(BackgroundSubagentError::ActiveConversation);
            }
            let guard = FinalizerCompletionGuard {
                supervisor: Arc::downgrade(self),
                attempt_id: attempt_id.clone(),
            };
            let future = future.take().ok_or_else(|| {
                BackgroundSubagentError::Durability(
                    "background finalizer future was already consumed".to_string(),
                )
            })?;
            let (start_tx, start_rx) = tokio::sync::oneshot::channel();
            let panic_supervisor = Arc::downgrade(self);
            let panic_attempt_id = attempt_id.clone();
            let handle = tokio::spawn(async move {
                let _guard = guard;
                if start_rx.await.is_err() {
                    return;
                }
                let outcome = std::panic::AssertUnwindSafe(future).catch_unwind().await;
                if outcome.is_err()
                    && let Some(supervisor) = panic_supervisor.upgrade()
                {
                    supervisor
                        .terminalize_panicked_finalizer(&panic_attempt_id)
                        .await;
                }
            });
            state.finalizer_handles.insert(attempt_id, handle);
            let _ = start_tx.send(());
            Ok(())
        })
    }

    async fn terminalize_panicked_finalizer(&self, attempt_id: &SubagentAttemptId) {
        let result = self
            .record_terminal_with_retry(
                attempt_id,
                BackgroundSubagentExecutionStatus::Failed,
                None,
                Some("background subagent finalizer panicked".to_string()),
            )
            .await;
        if let Some(result) = result {
            self.enqueue_terminal_fallback_and_notify(&result);
        }
    }

    fn finish_finalizer(&self, attempt_id: &SubagentAttemptId) {
        self.with_state(|state| {
            if state.finalizer_handles.remove(attempt_id).is_none() {
                state.completed_finalizers.insert(attempt_id.clone());
            }
        });
        self.notify.notify_waiters();
    }

    async fn drain_finalizers_until(&self, deadline: tokio::time::Instant) -> bool {
        let handles = self.with_state(|state| std::mem::take(&mut state.finalizer_handles));
        let mut draining = FinalizerDrainGuard {
            supervisor: self,
            handles,
        };
        let attempt_ids = draining.handles.keys().cloned().collect::<Vec<_>>();
        let mut timed_out = false;
        for attempt_id in attempt_ids {
            let Some(finalizer) = draining.handles.get_mut(&attempt_id) else {
                continue;
            };
            if tokio::time::timeout_at(deadline, finalizer).await.is_ok() {
                draining.handles.remove(&attempt_id);
                self.with_state(|state| {
                    state.completed_finalizers.remove(&attempt_id);
                });
            } else {
                timed_out = true;
                if let Some(finalizer) = draining.handles.get(&attempt_id) {
                    finalizer.abort();
                }
            }
        }
        drop(draining);
        timed_out
    }

    async fn drain_durable_store_until(
        &self,
        deadline: tokio::time::Instant,
    ) -> Result<(), BackgroundSubagentError> {
        let Some(store) = self.durable_store.as_ref() else {
            return Ok(());
        };
        match tokio::time::timeout_at(deadline, store.drain_background_subagent_operations()).await
        {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(BackgroundSubagentError::Durability(error.to_string())),
            Err(_) => Err(shutdown_deadline_error()),
        }
    }

    pub(crate) async fn transition_durable(
        &self,
        attempt_id: &SubagentAttemptId,
        status: BackgroundSubagentExecutionStatus,
    ) -> Result<(), BackgroundSubagentError> {
        let mut info = self
            .with_state(|state| {
                state
                    .active
                    .get(attempt_id)
                    .map(|active| active.info.clone())
            })
            .ok_or(BackgroundSubagentError::NotFound)?;
        info.execution_status = status;
        info.updated_at = Utc::now();
        self.persist_execution_info(&info).await?;
        self.with_state(|state| {
            if let Some(active) = state.active.get_mut(attempt_id) {
                active.info = info;
            }
        });
        self.notify.notify_waiters();
        Ok(())
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

    async fn persist_execution_info(
        &self,
        info: &BackgroundSubagentTaskInfo,
    ) -> Result<(), BackgroundSubagentError> {
        let Some(store) = self.durable_store.as_ref() else {
            return Ok(());
        };
        let mut record = store
            .load_background_subagent(&info.attempt_id)
            .await
            .map_err(|error| BackgroundSubagentError::Durability(error.to_string()))?;
        record.owner_lease.host_instance_id = self.durable_host_instance_id.clone();
        record.owner_lease.fencing_generation = self.durable_fencing_generation;
        record.execution_status = durable_execution_status(info.execution_status);
        record.child_run_id.clone_from(&info.child_run_id);
        record.updated_at = info.updated_at;
        store
            .update_background_subagent_execution(record)
            .await
            .map_err(|error| BackgroundSubagentError::Durability(error.to_string()))?;
        Ok(())
    }

    async fn heartbeat_durable(
        &self,
        attempt_id: &SubagentAttemptId,
    ) -> Result<(), SessionStoreError> {
        let Some(store) = self.durable_store.as_ref() else {
            return Ok(());
        };
        #[cfg(test)]
        if self
            .heartbeat_failures_remaining
            .fetch_update(
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
                |remaining| remaining.checked_sub(1),
            )
            .is_ok()
        {
            return Err(SessionStoreError::Failed(
                "synthetic heartbeat failure".to_string(),
            ));
        }
        let lease_expires_at = Utc::now()
            + chrono::Duration::from_std(self.durable_owner_lease_ttl)
                .map_err(|error| SessionStoreError::Failed(error.to_string()))?;
        store
            .heartbeat_background_subagent(
                attempt_id,
                &self.durable_host_instance_id,
                self.durable_fencing_generation,
                lease_expires_at,
            )
            .await?;
        Ok(())
    }

    pub(super) fn durable_heartbeat_interval(&self) -> Option<Duration> {
        self.durable_store.as_ref()?;
        Some(
            self.durable_owner_lease_ttl
                .div_f64(3.0)
                .max(Duration::from_millis(1)),
        )
    }

    pub(super) fn durable_transient_retry_delay(&self) -> Duration {
        self.durable_heartbeat_interval()
            .unwrap_or(Duration::from_millis(250))
            .min(Duration::from_millis(250))
            .max(Duration::from_millis(1))
    }

    fn durable_retry_delay_cap(&self) -> Duration {
        self.durable_heartbeat_interval()
            .unwrap_or(Duration::from_secs(5))
            .min(Duration::from_secs(5))
            .max(Duration::from_millis(1))
    }

    pub(crate) async fn record_terminal_with_retry(
        &self,
        attempt_id: &SubagentAttemptId,
        status: BackgroundSubagentExecutionStatus,
        content: Option<String>,
        error: Option<String>,
    ) -> Option<BackgroundSubagentTaskResult> {
        let delay_cap = self.durable_retry_delay_cap();
        let mut delay = Duration::from_millis(25).min(delay_cap);
        loop {
            match self
                .record_terminal_durable(attempt_id, status, content.clone(), error.clone())
                .await
            {
                Ok(result) => return result,
                Err(DurableTerminalPersistenceError::ConfirmedOwnerLoss) => {
                    self.abandon_after_owner_loss(attempt_id);
                    return None;
                }
                Err(DurableTerminalPersistenceError::Retryable(_)) => {}
            }
            match self.heartbeat_durable_with_retry(attempt_id).await {
                DurableLeaseRefresh::Renewed | DurableLeaseRefresh::RetryableFailure => {}
                DurableLeaseRefresh::TerminalObserved => {
                    tokio::time::sleep(self.durable_transient_retry_delay()).await;
                    continue;
                }
                DurableLeaseRefresh::ConfirmedOwnerLoss => {
                    self.abandon_after_owner_loss(attempt_id);
                    return None;
                }
            }
            tokio::time::sleep(delay).await;
            delay = delay.saturating_mul(2).min(delay_cap);
        }
    }

    pub(super) async fn heartbeat_durable_with_retry(
        &self,
        attempt_id: &SubagentAttemptId,
    ) -> DurableLeaseRefresh {
        let delay_cap = self.durable_retry_delay_cap();
        let mut delay = Duration::from_millis(25).min(delay_cap);
        for attempt in 0..3 {
            match self.heartbeat_durable(attempt_id).await {
                Ok(()) => return DurableLeaseRefresh::Renewed,
                Err(SessionStoreError::Conflict(_) | SessionStoreError::NotFound(_)) => {
                    return self.classify_durable_lease(attempt_id).await;
                }
                Err(_) if attempt < 2 => {
                    tokio::time::sleep(delay).await;
                    delay = delay.saturating_mul(4).min(delay_cap);
                }
                Err(_) => return DurableLeaseRefresh::RetryableFailure,
            }
        }
        DurableLeaseRefresh::RetryableFailure
    }

    async fn classify_durable_lease(&self, attempt_id: &SubagentAttemptId) -> DurableLeaseRefresh {
        let Some(store) = self.durable_store.as_ref() else {
            return DurableLeaseRefresh::Renewed;
        };
        match store.load_background_subagent(attempt_id).await {
            Ok(record) if record.execution_status.is_terminal() => {
                DurableLeaseRefresh::TerminalObserved
            }
            Ok(record)
                if record.owner_lease.host_instance_id != self.durable_host_instance_id
                    || record.owner_lease.fencing_generation != self.durable_fencing_generation
                    || record.owner_lease.expired_at(Utc::now()) =>
            {
                DurableLeaseRefresh::ConfirmedOwnerLoss
            }
            Err(SessionStoreError::NotFound(_)) => DurableLeaseRefresh::ConfirmedOwnerLoss,
            Ok(_) | Err(_) => DurableLeaseRefresh::RetryableFailure,
        }
    }

    async fn classify_terminal_store_error(
        &self,
        attempt_id: &SubagentAttemptId,
        error: SessionStoreError,
    ) -> DurableTerminalPersistenceError {
        if matches!(
            error,
            SessionStoreError::NotFound(_) | SessionStoreError::Conflict(_)
        ) && matches!(
            self.classify_durable_lease(attempt_id).await,
            DurableLeaseRefresh::TerminalObserved | DurableLeaseRefresh::ConfirmedOwnerLoss
        ) {
            return DurableTerminalPersistenceError::ConfirmedOwnerLoss;
        }
        DurableTerminalPersistenceError::Retryable(BackgroundSubagentError::Durability(
            error.to_string(),
        ))
    }

    pub(crate) fn abandon_after_owner_loss(&self, attempt_id: &SubagentAttemptId) {
        self.with_state(|state| {
            if let Some(active) = state.active.remove(attempt_id) {
                state.active_by_agent.remove(&active.info.agent_id);
            }
        });
        self.notify.notify_waiters();
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

    /// Hydrate one undelivered durable terminal result into this supervisor scope.
    ///
    /// Returns `true` when a new in-process projection was installed.
    ///
    /// # Errors
    ///
    /// Returns a durability error when the record is not terminal and undelivered.
    #[allow(clippy::too_many_lines)]
    pub fn hydrate_durable_result(
        &self,
        record: &BackgroundSubagentRecord,
        resolved_content: Option<String>,
    ) -> Result<bool, BackgroundSubagentError> {
        if !record.execution_status.is_terminal()
            || record.delivery_status != DurableBackgroundSubagentDeliveryStatus::Undelivered
        {
            return Err(BackgroundSubagentError::Durability(
                "only undelivered durable terminal results can be hydrated".to_string(),
            ));
        }
        let result_ref = record.result_ref.as_ref().ok_or_else(|| {
            BackgroundSubagentError::Durability(
                "durable terminal result is missing its result reference".to_string(),
            )
        })?;
        if record.retention_status == DurableBackgroundSubagentRetentionStatus::Artifact {
            let content = resolved_content.as_deref().ok_or_else(|| {
                BackgroundSubagentError::Durability(
                    "artifact-retained result requires resolved complete content".to_string(),
                )
            })?;
            if content.len() > self.limits.max_artifact_result_bytes
                || result_ref.size_bytes != u64::try_from(content.len()).unwrap_or(u64::MAX)
                || result_ref.digest.as_deref()
                    != Some(BackgroundSubagentArtifact::content_digest(content).as_str())
            {
                return Err(BackgroundSubagentError::Durability(
                    "resolved background-result artifact failed integrity or size policy"
                        .to_string(),
                ));
            }
        }
        let result = BackgroundSubagentTaskResult {
            attempt_id: record.attempt_id.clone(),
            agent_id: record.agent_id.clone(),
            subagent_name: record.subagent_name.clone(),
            linked_task_id: record.linked_task_id.clone(),
            status: background_execution_status(record.execution_status),
            parent_session_id: Some(record.parent_session_id.clone()),
            parent_run_id: Some(record.parent_run_id.clone()),
            child_run_id: record.child_run_id.clone(),
            content: resolved_content.or_else(|| result_ref.content.clone()),
            error: result_ref.error.clone(),
            failure_category: record.failure_category.clone(),
            cancellation_reason: record.cancellation_reason.clone(),
            delivery_status: BackgroundSubagentDeliveryStatus::Undelivered,
            delivery_claim: None,
            delivered_claim_id: None,
            retention_status: background_retention_status(record.retention_status),
            artifact_ref: result_ref.artifact_ref.clone(),
            retention_expires_at: record.retention_expires_at.unwrap_or(record.updated_at),
            logical_size_bytes: result_ref.size_bytes,
            digest: result_ref.digest.clone(),
            completed_at: record.terminal_at.unwrap_or(record.updated_at),
        };
        let captured_now = Utc::now();
        let installed = self.with_state(|state| {
            if state.active.contains_key(&record.attempt_id)
                || state.results.contains_key(&record.attempt_id)
            {
                return Ok(false);
            }
            if result.retention_status == BackgroundSubagentRetentionStatus::Artifact
                && result.retention_expires_at > captured_now
            {
                let retained_artifact_bytes =
                    projected_retained_artifact_bytes(state, captured_now);
                let result_size = usize::try_from(result.logical_size_bytes).unwrap_or(usize::MAX);
                if retained_artifact_bytes.saturating_add(result_size)
                    > self.limits.max_retained_artifact_bytes
                {
                    return Err(BackgroundSubagentError::QuotaExceeded);
                }
            }
            state
                .conversations
                .insert(record.agent_id.clone(), record.subagent_name.clone());
            state
                .results
                .insert(record.attempt_id.clone(), result.clone());
            trim_results(state, self.limits.max_retained_results);
            Ok(true)
        })?;
        if installed {
            self.enqueue_terminal_fallback_and_notify(&result);
        }
        Ok(installed)
    }

    #[allow(clippy::needless_pass_by_value)]
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
                content.as_deref(),
                error.as_deref(),
                self.limits.max_inline_result_bytes,
                self.limits.retained_result_ttl,
            );
            result.cancellation_reason = active.cancellation_reason;
            state.results.insert(attempt_id.clone(), result.clone());
            trim_results(state, self.limits.max_retained_results);
            Some(result)
        });
        self.notify.notify_waiters();
        result
    }

    #[allow(clippy::too_many_lines)]
    async fn record_terminal_durable(
        &self,
        attempt_id: &SubagentAttemptId,
        status: BackgroundSubagentExecutionStatus,
        mut content: Option<String>,
        mut error: Option<String>,
    ) -> Result<Option<BackgroundSubagentTaskResult>, DurableTerminalPersistenceError> {
        if self.durable_store.is_none() {
            return Ok(self.record_terminal(attempt_id, status, content, error));
        }
        if let Some(existing) = self.task_result(attempt_id) {
            return Ok(Some(existing));
        }
        #[cfg(test)]
        {
            let delay_millis = self
                .terminal_persistence_delay_millis
                .swap(0, std::sync::atomic::Ordering::SeqCst);
            if delay_millis > 0 {
                tokio::time::sleep(Duration::from_millis(
                    u64::try_from(delay_millis).unwrap_or(u64::MAX),
                ))
                .await;
            }
        }
        let captured_now = Utc::now();
        let pending = self.with_state(|state| {
            if let Some(pending) = state
                .active
                .get(attempt_id)
                .and_then(|active| active.pending_terminal.clone())
            {
                return Ok(Some(pending));
            }
            let artifact_size = (status == BackgroundSubagentExecutionStatus::Completed)
                .then(|| content.as_ref().map(String::len))
                .flatten()
                .filter(|size| *size > self.limits.max_inline_result_bytes);
            let retained_artifact_bytes =
                projected_retained_artifact_bytes(state, captured_now);
            let artifact_allowed = artifact_size.is_none_or(|size| {
                size <= self.limits.max_artifact_result_bytes
                    && retained_artifact_bytes.saturating_add(size)
                        <= self.limits.max_retained_artifact_bytes
            });
            let mut effective_status = status;
            if !artifact_allowed {
                let logical_size = artifact_size.unwrap_or_default();
                effective_status = BackgroundSubagentExecutionStatus::Failed;
                content = None;
                error = Some(format!(
                    "background subagent result exceeded host artifact retention limits (logical_size_bytes: {logical_size})"
                ));
            }
            let Some(active) = state.active.get_mut(attempt_id) else {
                return Ok(None);
            };
            let mut result = BackgroundSubagentTaskResult::terminal(
                &active.info,
                effective_status,
                content.as_deref(),
                error.as_deref(),
                self.limits.max_inline_result_bytes,
                self.limits.retained_result_ttl,
            );
            result
                .cancellation_reason
                .clone_from(&active.cancellation_reason);
            let artifact = if artifact_size.is_some() && artifact_allowed {
                durable_result_artifact(
                    &self.durable_namespace,
                    attempt_id,
                    &mut result,
                    content.take(),
                )?
            } else {
                None
            };
            let pending = PendingTerminalEvidence { result, artifact };
            active.pending_terminal = Some(pending.clone());
            Ok(Some(pending))
        })
        .map_err(DurableTerminalPersistenceError::Retryable)?;
        let Some(pending) = pending else {
            return Ok(None);
        };
        let result = pending.result;
        let Some(store) = self.durable_store.as_ref() else {
            return Err(DurableTerminalPersistenceError::Retryable(
                BackgroundSubagentError::Durability(
                    "durable store disappeared during terminal persistence".to_string(),
                ),
            ));
        };
        let mut record = store
            .load_background_subagent(attempt_id)
            .await
            .map_err(durable_terminal_load_error)?;
        record.owner_lease.host_instance_id = self.durable_host_instance_id.clone();
        record.owner_lease.fencing_generation = self.durable_fencing_generation;
        record.execution_status = durable_execution_status(result.status);
        record.child_run_id.clone_from(&result.child_run_id);
        record.result_ref = Some(durable_result_ref(&result));
        record.failure_category.clone_from(&result.failure_category);
        record
            .cancellation_reason
            .clone_from(&result.cancellation_reason);
        record.delivery_status = DurableBackgroundSubagentDeliveryStatus::Undelivered;
        record.delivery_claim = None;
        record.delivered_claim_id = None;
        record.retention_status = durable_retention_status(result.retention_status);
        record.retention_expires_at = Some(result.retention_expires_at);
        record.terminal_at = Some(result.completed_at);
        record.updated_at = result.completed_at;
        let artifact_size = pending
            .artifact
            .as_ref()
            .map(|artifact| artifact.size_bytes);
        let artifact_limits = pending
            .artifact
            .as_ref()
            .map(|_| BackgroundSubagentArtifactLimits {
                max_single_bytes: u64::try_from(self.limits.max_artifact_result_bytes)
                    .unwrap_or(u64::MAX),
                max_retained_bytes: u64::try_from(self.limits.max_retained_artifact_bytes)
                    .unwrap_or(u64::MAX),
            });
        #[cfg(test)]
        if self
            .terminal_commit_failures_remaining
            .fetch_update(
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
                |remaining| remaining.checked_sub(1),
            )
            .is_ok()
        {
            return Err(DurableTerminalPersistenceError::Retryable(
                BackgroundSubagentError::Durability(
                    "synthetic terminal commit failure".to_string(),
                ),
            ));
        }
        let terminal_commit = store
            .commit_background_subagent_terminal(BackgroundSubagentTerminalCommit {
                record,
                artifact: pending.artifact,
                artifact_limits,
            })
            .await;
        #[cfg(test)]
        if terminal_commit.is_ok()
            && self
                .terminal_commit_response_failures_remaining
                .fetch_update(
                    std::sync::atomic::Ordering::SeqCst,
                    std::sync::atomic::Ordering::SeqCst,
                    |remaining| remaining.checked_sub(1),
                )
                .is_ok()
        {
            return Err(DurableTerminalPersistenceError::Retryable(
                BackgroundSubagentError::Durability(
                    "synthetic terminal commit response failure".to_string(),
                ),
            ));
        }
        if let (Err(SessionStoreError::QuotaExceeded(_)), Some(artifact_size)) =
            (&terminal_commit, artifact_size)
        {
            self.with_state(|state| {
                if let Some(active) = state.active.get_mut(attempt_id) {
                    active.pending_terminal = None;
                }
            });
            return Box::pin(self.record_terminal_durable(
                attempt_id,
                BackgroundSubagentExecutionStatus::Failed,
                None,
                Some(format!(
                    "background subagent result exceeded durable artifact retention quota (logical_size_bytes: {artifact_size})"
                )),
            ))
            .await;
        }
        if let Err(error) = terminal_commit {
            return Err(self.classify_terminal_store_error(attempt_id, error).await);
        }
        let committed = self.with_state(|state| {
            if let Some(existing) = state.results.get(attempt_id) {
                return Some(existing.clone());
            }
            let active = state.active.remove(attempt_id)?;
            state.active_by_agent.remove(&active.info.agent_id);
            state.results.insert(attempt_id.clone(), result.clone());
            trim_results(state, self.limits.max_retained_results);
            Some(result)
        });
        self.notify.notify_waiters();
        Ok(committed)
    }

    pub(crate) fn terminal_result_message(
        &self,
        result: &BackgroundSubagentTaskResult,
        target_agent_id: &str,
    ) -> BusMessage {
        let message_text = match (&result.content, &result.error) {
            (Some(output), _) => output.clone(),
            (_, Some(error)) => format!(
                "Background delegate '{}' (agent_id: {}, attempt_id: {}) {}: {error}",
                result.subagent_name,
                result.agent_id,
                result.attempt_id.as_str(),
                result.status.as_str(),
            ),
            _ => format!(
                "Background delegate '{}' (agent_id: {}, attempt_id: {}) {}",
                result.subagent_name,
                result.agent_id,
                result.attempt_id.as_str(),
                result.status.as_str(),
            ),
        };
        BusMessage::text(message_text, result.agent_id.clone())
            .with_id(self.get_task_result_message_id(&result.attempt_id))
            .with_target(target_agent_id)
    }

    pub(crate) fn enqueue_terminal_fallback_and_notify(
        &self,
        result: &BackgroundSubagentTaskResult,
    ) {
        let message = self.terminal_result_message(result, "main");
        self.enqueue_message(result.attempt_id.clone(), message);
        self.notify_completion(&result.attempt_id);
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
            let duplicate = match active.steering_ids.get(&steering_id) {
                Some(existing) if existing == &message => true,
                Some(_) => return Err(BackgroundSubagentError::IdempotencyConflict),
                None => false,
            };
            if !duplicate && active.steering_ids.len() >= self.limits.max_operation_ids_per_attempt
            {
                return Err(BackgroundSubagentError::OperationHistoryFull);
            }
            if !duplicate {
                active
                    .steering_ids
                    .insert(steering_id.clone(), message.clone());
            }
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
        let (agent_id, cancellation, terminal_status) = self.with_state(|state| {
            if let Some(result) = state.results.get(attempt_id) {
                return Ok((
                    result.agent_id.clone(),
                    None,
                    Some(result.status.as_str().to_string()),
                ));
            }
            let active = state
                .active
                .get_mut(attempt_id)
                .ok_or(BackgroundSubagentError::NotFound)?;
            let duplicate = match active.cancellation_ids.get(&cancellation_id) {
                Some(existing) if existing == &reason => true,
                Some(_) => return Err(BackgroundSubagentError::IdempotencyConflict),
                None => false,
            };
            if !duplicate
                && active.cancellation_ids.len() >= self.limits.max_operation_ids_per_attempt
            {
                return Err(BackgroundSubagentError::OperationHistoryFull);
            }
            if !duplicate {
                active
                    .cancellation_ids
                    .insert(cancellation_id.clone(), reason.clone());
            }
            if !duplicate && active.cancellation_reason.is_none() {
                active.cancellation_reason.clone_from(&reason);
            }
            Ok::<_, BackgroundSubagentError>((
                active.info.agent_id.clone(),
                Some(active.control.cancellation.clone()),
                None,
            ))
        })?;
        if let Some(status) = terminal_status {
            return Ok(BackgroundSubagentCancellationReceipt {
                attempt_id: attempt_id.clone(),
                agent_id,
                cancellation_id,
                status,
            });
        }
        let Some(cancellation) = cancellation else {
            return Err(BackgroundSubagentError::NotFound);
        };
        cancellation.cancel();
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
                    if result.delivery_claim.as_ref() == Some(&claim) {
                        Ok(result.clone())
                    } else if result
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

    /// Claim delivery through durable compare-and-set before updating in-process state.
    ///
    /// # Errors
    ///
    /// Returns an error when durable or in-process claim ownership cannot be acquired.
    pub async fn claim_delivery_durable(
        &self,
        attempt_id: &SubagentAttemptId,
        claim: BackgroundSubagentDeliveryClaim,
    ) -> Result<BackgroundSubagentTaskResult, BackgroundSubagentError> {
        if let Some(store) = self.durable_store.as_ref() {
            store
                .claim_background_subagent_delivery(
                    attempt_id,
                    DurableBackgroundSubagentDeliveryClaim {
                        claim_id: claim.claim_id.clone(),
                        continuation_run_id: claim.continuation_run_id.clone(),
                        deadline: claim.deadline,
                    },
                )
                .await
                .map_err(|error| BackgroundSubagentError::Durability(error.to_string()))?;
        }
        self.claim_delivery(attempt_id, claim)
    }

    /// Acknowledge delivery durably before releasing in-process retained content.
    ///
    /// # Errors
    ///
    /// Returns an error when the claim does not own delivery or durability fails.
    pub async fn acknowledge_delivery_durable(
        &self,
        attempt_id: &SubagentAttemptId,
        claim_id: &str,
    ) -> Result<(), BackgroundSubagentError> {
        if let Some(store) = self.durable_store.as_ref() {
            store
                .acknowledge_background_subagent_delivery(attempt_id, claim_id)
                .await
                .map_err(|error| BackgroundSubagentError::Durability(error.to_string()))?;
        }
        self.acknowledge_delivery(attempt_id, claim_id)
    }

    /// Release a failed pre-admission claim in durable and in-process state.
    ///
    /// # Errors
    ///
    /// Returns an error when the claim does not own delivery or durability fails.
    pub async fn release_delivery_claim_durable(
        &self,
        attempt_id: &SubagentAttemptId,
        claim_id: &str,
    ) -> Result<(), BackgroundSubagentError> {
        if let Some(store) = self.durable_store.as_ref() {
            store
                .release_background_subagent_delivery(
                    attempt_id,
                    claim_id,
                    DurableBackgroundSubagentDeliveryRelease::Retryable,
                )
                .await
                .map_err(|error| BackgroundSubagentError::Durability(error.to_string()))?;
        }
        self.release_delivery_claim(attempt_id, claim_id)
    }

    /// Commit or release all durable result claims owned by one parent run.
    ///
    /// Hosts call this only after the parent run's terminal evidence is durable. A committed
    /// parent consumes its claims; a failed or cancelled parent releases them for redelivery.
    ///
    /// # Errors
    ///
    /// Returns an error when durable claim acknowledgement or release fails.
    pub async fn finalize_parent_deliveries(
        &self,
        run_id: &RunId,
        committed: bool,
    ) -> Result<usize, BackgroundSubagentError> {
        let claims = self.with_state(|state| {
            state
                .results
                .iter()
                .filter_map(|(attempt_id, result)| {
                    let claim = result.delivery_claim.as_ref()?;
                    (result.delivery_status == BackgroundSubagentDeliveryStatus::Claimed
                        && claim.continuation_run_id.as_ref() == Some(run_id))
                    .then(|| (attempt_id.clone(), claim.claim_id.clone()))
                })
                .collect::<Vec<_>>()
        });
        for (attempt_id, claim_id) in &claims {
            if committed {
                self.acknowledge_delivery_durable(attempt_id, claim_id)
                    .await?;
            } else if let Some(store) = self.durable_store.as_ref() {
                store
                    .release_background_subagent_delivery(
                        attempt_id,
                        claim_id,
                        DurableBackgroundSubagentDeliveryRelease::ConsumerTerminated {
                            run_id: run_id.clone(),
                        },
                    )
                    .await
                    .map_err(|error| BackgroundSubagentError::Durability(error.to_string()))?;
                self.release_delivery_claim(attempt_id, claim_id)?;
            } else {
                self.release_delivery_claim_durable(attempt_id, claim_id)
                    .await?;
            }
        }
        Ok(claims.len())
    }

    /// Mirror a host-atomically admitted durable continuation into volatile delivery state.
    ///
    /// # Errors
    ///
    /// Returns an error when the result is absent or another claim already delivered it.
    pub fn mark_delivery_from_host(
        &self,
        attempt_id: &SubagentAttemptId,
        claim_id: &str,
        continuation_run_id: &RunId,
    ) -> Result<(), BackgroundSubagentError> {
        self.with_state(|state| {
            let result = state
                .results
                .get_mut(attempt_id)
                .ok_or(BackgroundSubagentError::NotFound)?;
            if result.delivery_status == BackgroundSubagentDeliveryStatus::Delivered {
                return if result.delivered_claim_id.as_deref() == Some(claim_id)
                    && result.delivery_claim.as_ref().is_none_or(|claim| {
                        claim.continuation_run_id.as_ref() == Some(continuation_run_id)
                    }) {
                    Ok(())
                } else {
                    Err(BackgroundSubagentError::Delivered)
                };
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

    pub(super) async fn claim_pending_messages_durable(
        &self,
        claim_scope: &str,
        continuation_run_id: Option<&RunId>,
    ) -> Vec<ClaimedBackgroundSubagentMessage> {
        if self.durable_store.is_none() {
            return self.claim_pending_messages(claim_scope, continuation_run_id);
        }
        let pending = self.with_state(|state| {
            state
                .pending_messages
                .iter()
                .map(|pending| (pending.attempt_id.clone(), pending.message.clone()))
                .collect::<Vec<_>>()
        });
        let mut claimed = Vec::new();
        for (attempt_id, message) in pending {
            let owned = self.with_state(|state| {
                state.results.get(&attempt_id).and_then(|result| {
                    result.delivery_claim.as_ref().and_then(|claim| {
                        (result.delivery_status == BackgroundSubagentDeliveryStatus::Claimed
                            && continuation_run_id.is_some()
                            && claim.continuation_run_id.as_ref() == continuation_run_id)
                            .then(|| claim.claim_id.clone())
                    })
                })
            });
            if let Some(claim_id) = owned {
                claimed.push(ClaimedBackgroundSubagentMessage {
                    attempt_id,
                    claim_id,
                    message,
                });
                continue;
            }
            let claim_id = format!(
                "bus:{claim_scope}:{}:{}",
                attempt_id.as_str(),
                uuid::Uuid::new_v4()
            );
            let claim = BackgroundSubagentDeliveryClaim {
                claim_id: claim_id.clone(),
                continuation_run_id: continuation_run_id.cloned(),
                deadline: Utc::now() + chrono::Duration::seconds(60),
            };
            if self
                .claim_delivery_durable(&attempt_id, claim)
                .await
                .is_ok()
            {
                claimed.push(ClaimedBackgroundSubagentMessage {
                    attempt_id,
                    claim_id,
                    message,
                });
            }
        }
        claimed
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

    /// Best-effort compatibility shutdown that discards deadline or durability errors.
    ///
    /// Product hosts that must prove complete cleanup should use [`Self::shutdown_checked`].
    pub async fn shutdown(&self, timeout: Option<Duration>) {
        let _ = self.shutdown_checked(timeout).await;
    }

    /// Stop admission and verify that every owned finalizer and terminal write is drained.
    ///
    /// A deadline error retains unfinished finalizer handles and store-owned operations. Keep the
    /// supervisor and runtime alive, then call this method again to finish draining them.
    ///
    /// # Errors
    ///
    /// Returns a durability error when terminal evidence cannot be persisted before the deadline.
    pub async fn shutdown_checked(
        &self,
        timeout: Option<Duration>,
    ) -> Result<(), BackgroundSubagentError> {
        let timeout = timeout.unwrap_or(self.limits.shutdown_grace);
        let started_at = tokio::time::Instant::now();
        let final_deadline = started_at + timeout;
        let _shutdown_guard = tokio::time::timeout_at(final_deadline, self.shutdown_gate.lock())
            .await
            .map_err(|_| shutdown_deadline_error())?;
        let controls = self.with_state(|state| {
            state.closing = true;
            state
                .active
                .values()
                .map(|active| active.control.cancellation.clone())
                .collect::<Vec<_>>()
        });
        for control in controls {
            control.cancel();
        }
        let cooperative_deadline = started_at + timeout.mul_f32(0.8);
        loop {
            let notified = self.notify.notified();
            let has_active_finalizer = self.with_state(|state| {
                state
                    .active
                    .keys()
                    .any(|attempt_id| state.finalizer_handles.contains_key(attempt_id))
            });
            if !has_active_finalizer || timeout.is_zero() {
                break;
            }
            if tokio::time::timeout_at(cooperative_deadline, notified)
                .await
                .is_err()
            {
                break;
            }
        }
        let active_finalizer_abort_handles = self.with_state(|state| {
            state
                .active
                .keys()
                .filter_map(|attempt_id| state.finalizer_handles.get(attempt_id))
                .map(JoinHandle::abort_handle)
                .collect::<Vec<_>>()
        });
        for abort_handle in active_finalizer_abort_handles {
            abort_handle.abort();
        }
        let mut shutdown_error = self
            .drain_finalizers_until(final_deadline)
            .await
            .then(shutdown_deadline_error);
        if let Err(error) = self.drain_durable_store_until(final_deadline).await {
            shutdown_error.get_or_insert(error);
        }
        if shutdown_error.is_none()
            && let Err(error) = self.force_interrupt_active(final_deadline).await
        {
            shutdown_error = Some(error);
        }
        if let Err(error) = self.drain_durable_store_until(final_deadline).await {
            shutdown_error.get_or_insert(error);
        }
        if self.drain_finalizers_until(final_deadline).await {
            shutdown_error.get_or_insert_with(shutdown_deadline_error);
        }
        let incomplete = self
            .with_state(|state| !state.active.is_empty() || !state.finalizer_handles.is_empty());
        if incomplete {
            shutdown_error.get_or_insert_with(shutdown_deadline_error);
        }
        shutdown_error.map_or(Ok(()), Err)
    }

    async fn force_interrupt_active(
        &self,
        deadline: tokio::time::Instant,
    ) -> Result<(), BackgroundSubagentError> {
        let attempt_ids = self.with_state(|state| state.active.keys().cloned().collect::<Vec<_>>());
        for attempt_id in attempt_ids {
            let result = loop {
                let terminal = self.record_terminal_durable(
                    &attempt_id,
                    BackgroundSubagentExecutionStatus::Cancelled,
                    None,
                    Some("background subagent interrupted during host shutdown".to_string()),
                );
                match tokio::time::timeout_at(deadline, terminal).await {
                    Ok(Ok(result)) => break result,
                    Ok(Err(DurableTerminalPersistenceError::ConfirmedOwnerLoss)) => {
                        self.abandon_after_owner_loss(&attempt_id);
                        break None;
                    }
                    Ok(Err(DurableTerminalPersistenceError::Retryable(error))) => {
                        if tokio::time::Instant::now() >= deadline {
                            return Err(error);
                        }
                        let retry_at =
                            (tokio::time::Instant::now() + Duration::from_millis(25)).min(deadline);
                        tokio::time::sleep_until(retry_at).await;
                        if tokio::time::Instant::now() >= deadline {
                            return Err(error);
                        }
                    }
                    Err(_) => {
                        return Err(BackgroundSubagentError::Durability(
                            "background subagent shutdown exceeded its terminalization deadline"
                                .to_string(),
                        ));
                    }
                }
            };
            let Some(result) = result else {
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
        Ok(())
    }
}

fn shutdown_deadline_error() -> BackgroundSubagentError {
    BackgroundSubagentError::Durability(
        "background subagent shutdown exceeded its terminalization deadline".to_string(),
    )
}

fn durable_terminal_load_error(error: SessionStoreError) -> DurableTerminalPersistenceError {
    match error {
        SessionStoreError::NotFound(_) => DurableTerminalPersistenceError::ConfirmedOwnerLoss,
        error => DurableTerminalPersistenceError::Retryable(BackgroundSubagentError::Durability(
            error.to_string(),
        )),
    }
}

fn durable_acceptance_record(
    info: &BackgroundSubagentTaskInfo,
    namespace_id: &str,
    profile: String,
    trace_context: TraceContext,
    host_instance_id: &str,
    fencing_generation: u64,
    lease_ttl: Duration,
) -> Result<BackgroundSubagentRecord, BackgroundSubagentError> {
    let parent_session_id = info.parent_session_id.clone().ok_or_else(|| {
        BackgroundSubagentError::Durability(
            "durable supervisor requires parent_session_id".to_string(),
        )
    })?;
    let parent_run_id = info.parent_run_id.clone().ok_or_else(|| {
        BackgroundSubagentError::Durability("durable supervisor requires parent_run_id".to_string())
    })?;
    let lease_expires_at = info.accepted_at
        + chrono::Duration::from_std(lease_ttl).map_err(|error| {
            BackgroundSubagentError::Durability(format!("invalid durable owner lease TTL: {error}"))
        })?;
    Ok(BackgroundSubagentRecord {
        schema_version: BACKGROUND_SUBAGENT_RECORD_VERSION,
        attempt_id: info.attempt_id.clone(),
        agent_id: info.agent_id.clone(),
        linked_task_id: info.linked_task_id.clone(),
        subagent_name: info.subagent_name.clone(),
        namespace_id: namespace_id.to_string(),
        parent_session_id,
        parent_run_id,
        child_run_id: info.child_run_id.clone(),
        continuation_run_id: None,
        profile,
        owner_lease: DurableBackgroundSubagentOwnerLease {
            host_instance_id: host_instance_id.to_string(),
            fencing_generation,
            heartbeat_at: info.accepted_at,
            lease_expires_at,
        },
        execution_status: durable_execution_status(info.execution_status),
        result_ref: None,
        failure_category: None,
        cancellation_reason: None,
        delivery_status: DurableBackgroundSubagentDeliveryStatus::Undelivered,
        delivery_claim: None,
        delivered_claim_id: None,
        automatic_continuation_suppressed_by_run_id: None,
        retention_status: DurableBackgroundSubagentRetentionStatus::Inline,
        retention_expires_at: None,
        trace_context: (!trace_context.is_empty()).then_some(trace_context),
        accepted_at: info.accepted_at,
        updated_at: info.updated_at,
        terminal_at: None,
    })
}

const fn background_execution_status(
    status: DurableBackgroundSubagentExecutionStatus,
) -> BackgroundSubagentExecutionStatus {
    match status {
        DurableBackgroundSubagentExecutionStatus::Accepted => {
            BackgroundSubagentExecutionStatus::Accepted
        }
        DurableBackgroundSubagentExecutionStatus::Starting => {
            BackgroundSubagentExecutionStatus::Starting
        }
        DurableBackgroundSubagentExecutionStatus::Running => {
            BackgroundSubagentExecutionStatus::Running
        }
        DurableBackgroundSubagentExecutionStatus::Waiting => {
            BackgroundSubagentExecutionStatus::Waiting
        }
        DurableBackgroundSubagentExecutionStatus::Completed => {
            BackgroundSubagentExecutionStatus::Completed
        }
        DurableBackgroundSubagentExecutionStatus::Failed => {
            BackgroundSubagentExecutionStatus::Failed
        }
        DurableBackgroundSubagentExecutionStatus::Cancelled => {
            BackgroundSubagentExecutionStatus::Cancelled
        }
    }
}

const fn durable_execution_status(
    status: BackgroundSubagentExecutionStatus,
) -> DurableBackgroundSubagentExecutionStatus {
    match status {
        BackgroundSubagentExecutionStatus::Accepted => {
            DurableBackgroundSubagentExecutionStatus::Accepted
        }
        BackgroundSubagentExecutionStatus::Starting => {
            DurableBackgroundSubagentExecutionStatus::Starting
        }
        BackgroundSubagentExecutionStatus::Running => {
            DurableBackgroundSubagentExecutionStatus::Running
        }
        BackgroundSubagentExecutionStatus::Waiting => {
            DurableBackgroundSubagentExecutionStatus::Waiting
        }
        BackgroundSubagentExecutionStatus::Completed => {
            DurableBackgroundSubagentExecutionStatus::Completed
        }
        BackgroundSubagentExecutionStatus::Failed => {
            DurableBackgroundSubagentExecutionStatus::Failed
        }
        BackgroundSubagentExecutionStatus::Cancelled => {
            DurableBackgroundSubagentExecutionStatus::Cancelled
        }
    }
}

fn durable_result_artifact(
    namespace_id: &str,
    attempt_id: &SubagentAttemptId,
    result: &mut BackgroundSubagentTaskResult,
    content: Option<String>,
) -> Result<Option<BackgroundSubagentArtifact>, BackgroundSubagentError> {
    let Some(content) = content else {
        return Ok(None);
    };
    let digest = result.digest.clone().ok_or_else(|| {
        BackgroundSubagentError::Durability(
            "oversized result is missing its content digest".to_string(),
        )
    })?;
    let artifact_ref = format!(
        "starweaver-background://{}/{}/{}",
        namespace_id,
        attempt_id.as_str(),
        digest.trim_start_matches("sha256:")
    );
    let artifact = BackgroundSubagentArtifact {
        artifact_ref: artifact_ref.clone(),
        namespace_id: namespace_id.to_string(),
        attempt_id: attempt_id.clone(),
        size_bytes: u64::try_from(content.len()).unwrap_or(u64::MAX),
        content,
        digest,
        created_at: result.completed_at,
        expires_at: result.retention_expires_at,
    };
    result.retention_status = BackgroundSubagentRetentionStatus::Artifact;
    result.artifact_ref = Some(artifact_ref);
    Ok(Some(artifact))
}

fn durable_result_ref(result: &BackgroundSubagentTaskResult) -> DurableBackgroundSubagentResultRef {
    DurableBackgroundSubagentResultRef {
        content: result.content.clone(),
        error: result.error.as_ref().map(|_| match result.status {
            BackgroundSubagentExecutionStatus::Cancelled => {
                "background subagent execution was cancelled".to_string()
            }
            _ => "background subagent execution failed".to_string(),
        }),
        artifact_ref: result.artifact_ref.clone(),
        digest: result.digest.clone(),
        size_bytes: result.logical_size_bytes,
    }
}

const fn background_retention_status(
    status: DurableBackgroundSubagentRetentionStatus,
) -> BackgroundSubagentRetentionStatus {
    match status {
        DurableBackgroundSubagentRetentionStatus::Inline => {
            BackgroundSubagentRetentionStatus::Inline
        }
        DurableBackgroundSubagentRetentionStatus::Artifact => {
            BackgroundSubagentRetentionStatus::Artifact
        }
        DurableBackgroundSubagentRetentionStatus::Expired => {
            BackgroundSubagentRetentionStatus::Expired
        }
    }
}

const fn durable_retention_status(
    status: BackgroundSubagentRetentionStatus,
) -> DurableBackgroundSubagentRetentionStatus {
    match status {
        BackgroundSubagentRetentionStatus::Inline => {
            DurableBackgroundSubagentRetentionStatus::Inline
        }
        BackgroundSubagentRetentionStatus::Artifact => {
            DurableBackgroundSubagentRetentionStatus::Artifact
        }
        BackgroundSubagentRetentionStatus::Expired => {
            DurableBackgroundSubagentRetentionStatus::Expired
        }
    }
}

fn projected_retained_artifact_bytes(
    state: &BackgroundSubagentState,
    captured_now: DateTime<Utc>,
) -> usize {
    state
        .results
        .values()
        .filter(|result| {
            result.retention_status == BackgroundSubagentRetentionStatus::Artifact
                && result.retention_expires_at > captured_now
        })
        .map(|result| usize::try_from(result.logical_size_bytes).unwrap_or(usize::MAX))
        .chain(state.active.values().filter_map(|active| {
            active
                .pending_terminal
                .as_ref()
                .and_then(|pending| pending.artifact.as_ref())
                .filter(|artifact| artifact.expires_at > captured_now)
                .map(|artifact| usize::try_from(artifact.size_bytes).unwrap_or(usize::MAX))
        }))
        .fold(0usize, usize::saturating_add)
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
        if let Some(removed) = state.results.remove(&oldest) {
            let agent_still_retained = state
                .active
                .values()
                .any(|active| active.info.agent_id == removed.agent_id)
                || state
                    .results
                    .values()
                    .any(|result| result.agent_id == removed.agent_id);
            if !agent_still_retained {
                state.conversations.remove(&removed.agent_id);
            }
        }
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

    use starweaver_core::ConversationId;
    use starweaver_session::{InMemorySessionStore, RunRecord, SessionRecord};

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

    async fn durable_parent(suffix: &str) -> (Arc<InMemorySessionStore>, SessionId, RunId) {
        let store = Arc::new(InMemorySessionStore::new());
        let session_id = SessionId::from_string(format!("session-{suffix}"));
        let run_id = RunId::from_string(format!("run-{suffix}"));
        let mut session = SessionRecord::new(session_id.clone());
        session.namespace_id = "test".to_string();
        session.profile = Some("test-profile".to_string());
        store.save_session(session).await.unwrap();
        let mut run = RunRecord::new(
            session_id.clone(),
            run_id.clone(),
            ConversationId::from_string(format!("conversation-{suffix}")),
        );
        run.profile = Some("test-profile".to_string());
        store.append_run(run).await.unwrap();
        (store, session_id, run_id)
    }

    async fn accept_durable_attempt(
        supervisor: &BackgroundSubagentSupervisor,
        attempt_id: &SubagentAttemptId,
        agent_id: &str,
        session_id: SessionId,
        run_id: RunId,
    ) {
        supervisor
            .accept_durable(BackgroundSubagentAcceptance {
                attempt_id: attempt_id.clone(),
                agent_id: agent_id.to_string(),
                subagent_name: "child".to_string(),
                linked_task_id: None,
                prompt: "bounded task".to_string(),
                parent_session_id: Some(session_id),
                parent_run_id: Some(run_id),
                is_resume: false,
            })
            .await
            .unwrap();
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
                Some("no longer needed".to_string()),
            )
            .unwrap();
        assert_eq!(first, duplicate);
        assert_eq!(
            supervisor
                .request_cancellation_with_reason(
                    &attempt,
                    "cancel-1".to_string(),
                    Some("different payload".to_string()),
                )
                .unwrap_err(),
            BackgroundSubagentError::IdempotencyConflict
        );
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
        let base_history = vec![ModelMessage::Response(
            starweaver_model::ModelResponse::text("prior child turn"),
        )];
        let mut completed_history = base_history.clone();
        completed_history.push(ModelMessage::Response(
            starweaver_model::ModelResponse::text("child result"),
        ));
        source
            .subagent_history
            .insert("child-bg-delta".to_string(), completed_history);
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
            &base_history,
        );
        supervisor.end_parent_run(&parent_run);
        supervisor.publish_committed_context_delta(&attempt, &handle, &delta);
        assert_eq!(handle.snapshot().usage.requests, 0);
        let mut before_terminal = AgentContext::default();
        supervisor.apply_context_deltas(&mut before_terminal);
        assert_eq!(before_terminal.usage.requests, 0);

        supervisor.record_terminal(
            &attempt,
            BackgroundSubagentExecutionStatus::Completed,
            Some("done".to_string()),
            None,
        );
        supervisor.publish_committed_context_delta(&attempt, &handle, &delta);
        let mut restored = AgentContext::default();
        restored
            .subagent_history
            .insert("child-bg-delta".to_string(), base_history);
        supervisor.apply_context_deltas(&mut restored);
        supervisor.apply_context_deltas(&mut restored);
        assert_eq!(restored.usage.requests, 2);
        assert_eq!(restored.subagent_history["child-bg-delta"].len(), 2);
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

        let started_at = tokio::time::Instant::now();
        supervisor
            .shutdown_checked(Some(Duration::from_millis(250)))
            .await
            .unwrap();

        assert!(started_at.elapsed() < Duration::from_millis(150));
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
    async fn completed_finalizers_are_reaped_without_shutdown() {
        let supervisor = Arc::new(BackgroundSubagentSupervisor::new());
        let attempt = SubagentAttemptId::from_string("subattempt_reaped_finalizer");
        accept(&supervisor, &attempt, "child-bg-reaped-finalizer", false).unwrap();
        let finalizing = supervisor.clone();
        let finalizing_attempt = attempt.clone();
        supervisor
            .spawn_finalizer(attempt.clone(), async move {
                finalizing.record_terminal(
                    &finalizing_attempt,
                    BackgroundSubagentExecutionStatus::Completed,
                    Some("done".to_string()),
                    None,
                );
            })
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let notified = supervisor.notify.notified();
                if supervisor.with_state(|state| state.finalizer_handles.is_empty()) {
                    break;
                }
                notified.await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            supervisor.task_result(&attempt).unwrap().status,
            BackgroundSubagentExecutionStatus::Completed
        );
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn panicked_finalizer_retries_durable_terminal_and_queues_before_callback() {
        let (store, session_id, run_id) = durable_parent("panicked-finalizer").await;
        let callback_count = Arc::new(AtomicUsize::new(0));
        let queued_before_callback = Arc::new(AtomicUsize::new(0));
        let supervisor_ref = Arc::new(Mutex::new(None::<Weak<BackgroundSubagentSupervisor>>));
        let counted = callback_count.clone();
        let observed_queue = queued_before_callback.clone();
        let callback_supervisor = supervisor_ref.clone();
        let supervisor = Arc::new(
            BackgroundSubagentSupervisor::new()
                .with_durable_store(store.clone(), "test")
                .with_completion_callback(Arc::new(
                    move |result: &BackgroundSubagentTaskResult| {
                        counted.fetch_add(1, Ordering::SeqCst);
                        let supervisor = callback_supervisor
                            .lock()
                            .unwrap()
                            .as_ref()
                            .and_then(Weak::upgrade);
                        if supervisor.is_some_and(|supervisor| {
                            let message_id =
                                supervisor.get_task_result_message_id(&result.attempt_id);
                            supervisor.with_state(|state| {
                                state.pending_messages.iter().any(|pending| {
                                    pending.attempt_id == result.attempt_id
                                        && pending.message.id == message_id
                                })
                            })
                        }) {
                            observed_queue.fetch_add(1, Ordering::SeqCst);
                        }
                    },
                )),
        );
        *supervisor_ref.lock().unwrap() = Some(Arc::downgrade(&supervisor));
        let attempt = SubagentAttemptId::from_string("subattempt_panicked_finalizer");
        accept_durable_attempt(
            &supervisor,
            &attempt,
            "child-bg-panicked-finalizer",
            session_id,
            run_id,
        )
        .await;
        supervisor
            .terminal_commit_failures_remaining
            .store(5, Ordering::SeqCst);
        supervisor
            .heartbeat_failures_remaining
            .store(8, Ordering::SeqCst);
        supervisor
            .spawn_finalizer(attempt.clone(), async move {
                panic!("synthetic finalizer panic");
            })
            .unwrap();

        tokio::time::timeout(Duration::from_secs(4), async {
            loop {
                let notified = supervisor.notify.notified();
                if supervisor.task_result(&attempt).is_some()
                    && supervisor.with_state(|state| state.finalizer_handles.is_empty())
                {
                    break;
                }
                notified.await;
            }
        })
        .await
        .unwrap();

        let result = supervisor.task_result(&attempt).unwrap();
        assert_eq!(result.status, BackgroundSubagentExecutionStatus::Failed);
        assert_eq!(
            result.error.as_deref(),
            Some("background subagent finalizer panicked")
        );
        assert!(!supervisor.has_active_tasks());
        assert_eq!(
            supervisor
                .terminal_commit_failures_remaining
                .load(Ordering::SeqCst),
            0
        );
        assert_eq!(
            supervisor
                .heartbeat_failures_remaining
                .load(Ordering::SeqCst),
            0
        );
        assert_eq!(callback_count.load(Ordering::SeqCst), 1);
        assert_eq!(queued_before_callback.load(Ordering::SeqCst), 1);
        let durable = store.load_background_subagent(&attempt).await.unwrap();
        assert_eq!(
            durable.execution_status,
            DurableBackgroundSubagentExecutionStatus::Failed
        );
        let pending = supervisor.with_state(|state| {
            state
                .pending_messages
                .front()
                .map(|pending| pending.message.clone())
        });
        let pending = pending.unwrap();
        assert_eq!(pending.id, supervisor.get_task_result_message_id(&attempt));
        assert_eq!(pending.target.as_deref(), Some("main"));
        assert!(pending.content_text().contains("finalizer panicked"));
    }

    #[tokio::test]
    async fn uncertain_terminal_commit_response_replays_stable_result() {
        let (store, session_id, run_id) = durable_parent("uncertain-terminal-response").await;
        let callback_count = Arc::new(AtomicUsize::new(0));
        let counted = callback_count.clone();
        let supervisor = Arc::new(
            BackgroundSubagentSupervisor::new()
                .with_durable_store(store.clone(), "test")
                .with_completion_callback(Arc::new(
                    move |_result: &BackgroundSubagentTaskResult| {
                        counted.fetch_add(1, Ordering::SeqCst);
                    },
                )),
        );
        let attempt = SubagentAttemptId::from_string("subattempt_uncertain_terminal_response");
        accept_durable_attempt(
            &supervisor,
            &attempt,
            "child-bg-uncertain-terminal-response",
            session_id,
            run_id,
        )
        .await;
        supervisor
            .terminal_commit_response_failures_remaining
            .store(1, Ordering::SeqCst);
        supervisor
            .spawn_finalizer(attempt.clone(), async move {
                panic!("synthetic uncertain terminal response");
            })
            .unwrap();

        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let notified = supervisor.notify.notified();
                if supervisor.task_result(&attempt).is_some()
                    && supervisor.with_state(|state| state.finalizer_handles.is_empty())
                {
                    break;
                }
                notified.await;
            }
        })
        .await
        .unwrap();

        assert!(!supervisor.has_active_tasks());
        assert_eq!(
            supervisor
                .terminal_commit_response_failures_remaining
                .load(Ordering::SeqCst),
            0
        );
        assert_eq!(callback_count.load(Ordering::SeqCst), 1);
        assert!(supervisor.has_pending_messages());
        assert_eq!(
            supervisor.task_result(&attempt).unwrap().status,
            BackgroundSubagentExecutionStatus::Failed
        );
        assert_eq!(
            store
                .load_background_subagent(&attempt)
                .await
                .unwrap()
                .execution_status,
            DurableBackgroundSubagentExecutionStatus::Failed
        );
    }

    #[tokio::test]
    async fn stale_owner_heartbeat_is_confirmed_before_active_state_is_abandoned() {
        let (store, session_id, run_id) = durable_parent("stale-owner-heartbeat").await;
        let supervisor = BackgroundSubagentSupervisor::new()
            .with_durable_store(store.clone(), "test")
            .with_durable_owner("short-lived-owner", 7, Duration::from_millis(100));
        let attempt = SubagentAttemptId::from_string("subattempt_stale_owner_heartbeat");
        accept_durable_attempt(
            &supervisor,
            &attempt,
            "child-bg-stale-owner-heartbeat",
            session_id,
            run_id,
        )
        .await;
        tokio::time::sleep(Duration::from_millis(125)).await;

        assert_eq!(
            supervisor.heartbeat_durable_with_retry(&attempt).await,
            DurableLeaseRefresh::ConfirmedOwnerLoss
        );
        assert!(supervisor.has_active_tasks());
        supervisor.abandon_after_owner_loss(&attempt);
        assert!(!supervisor.has_active_tasks());
        assert_eq!(
            store
                .load_background_subagent(&attempt)
                .await
                .unwrap()
                .execution_status,
            DurableBackgroundSubagentExecutionStatus::Accepted
        );
    }

    #[tokio::test]
    async fn shutdown_treats_confirmed_owner_loss_as_drained() {
        let (store, session_id, run_id) = durable_parent("shutdown-owner-loss").await;
        let supervisor = BackgroundSubagentSupervisor::new()
            .with_durable_store(store, "test")
            .with_durable_owner("shutdown-owner", 11, Duration::from_millis(100));
        let attempt = SubagentAttemptId::from_string("subattempt_shutdown_owner_loss");
        accept_durable_attempt(
            &supervisor,
            &attempt,
            "child-bg-shutdown-owner-loss",
            session_id,
            run_id,
        )
        .await;
        tokio::time::sleep(Duration::from_millis(125)).await;
        let started_at = tokio::time::Instant::now();
        supervisor
            .shutdown_checked(Some(Duration::from_millis(250)))
            .await
            .unwrap();

        assert!(started_at.elapsed() < Duration::from_millis(400));
        assert!(!supervisor.has_active_tasks());
    }

    #[tokio::test]
    async fn shutdown_deadline_bounds_pending_terminal_persistence() {
        let (store, session_id, run_id) = durable_parent("shutdown-terminal-deadline").await;
        let supervisor =
            BackgroundSubagentSupervisor::new().with_durable_store(store.clone(), "test");
        let attempt = SubagentAttemptId::from_string("subattempt_shutdown_terminal_deadline");
        accept_durable_attempt(
            &supervisor,
            &attempt,
            "child-bg-shutdown-terminal-deadline",
            session_id,
            run_id,
        )
        .await;
        supervisor
            .terminal_persistence_delay_millis
            .store(500, Ordering::SeqCst);
        let started_at = tokio::time::Instant::now();

        assert!(
            supervisor
                .shutdown_checked(Some(Duration::from_millis(50)))
                .await
                .is_err()
        );
        assert!(started_at.elapsed() < Duration::from_millis(250));
        assert!(supervisor.has_active_tasks());
        assert!(supervisor.with_state(|state| state.finalizer_handles.is_empty()));

        supervisor
            .shutdown_checked(Some(Duration::from_millis(50)))
            .await
            .unwrap();
        assert!(!supervisor.has_active_tasks());
        assert_eq!(
            store
                .load_background_subagent(&attempt)
                .await
                .unwrap()
                .execution_status,
            DurableBackgroundSubagentExecutionStatus::Cancelled
        );
    }

    #[tokio::test]
    async fn checked_shutdown_drops_the_owned_finalizer_future_before_returning() {
        struct DropCounter(Arc<AtomicUsize>);

        impl Drop for DropCounter {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }

        let supervisor = Arc::new(BackgroundSubagentSupervisor::new());
        let attempt = SubagentAttemptId::from_string("subattempt_shutdown_owned_future");
        accept(
            &supervisor,
            &attempt,
            "child-bg-shutdown-owned-future",
            false,
        )
        .unwrap();
        let dropped = Arc::new(AtomicUsize::new(0));
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let worker_drop = dropped.clone();
        supervisor
            .spawn_finalizer(attempt.clone(), async move {
                let _drop_counter = DropCounter(worker_drop);
                let _ = started_tx.send(());
                std::future::pending::<()>().await;
            })
            .unwrap();
        started_rx.await.unwrap();

        supervisor
            .shutdown_checked(Some(Duration::from_millis(100)))
            .await
            .unwrap();

        assert_eq!(dropped.load(Ordering::SeqCst), 1);
        assert!(!supervisor.has_active_tasks());
        assert!(supervisor.with_state(|state| state.finalizer_handles.is_empty()));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_timeout_retains_an_uncooperative_finalizer_for_later_drain() {
        let supervisor = Arc::new(BackgroundSubagentSupervisor::new());
        let attempt = SubagentAttemptId::from_string("subattempt_shutdown_uncooperative");
        accept(
            &supervisor,
            &attempt,
            "child-bg-shutdown-uncooperative",
            false,
        )
        .unwrap();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        supervisor
            .spawn_finalizer(attempt.clone(), async move {
                let _ = started_tx.send(());
                std::thread::sleep(Duration::from_millis(250));
                std::future::pending::<()>().await;
            })
            .unwrap();
        started_rx.await.unwrap();
        let started_at = tokio::time::Instant::now();

        assert!(
            supervisor
                .shutdown_checked(Some(Duration::from_millis(40)))
                .await
                .is_err()
        );
        assert!(started_at.elapsed() < Duration::from_millis(150));
        assert!(supervisor.with_state(|state| {
            state.finalizer_handles.contains_key(&attempt)
                && !state.completed_finalizers.contains(&attempt)
        }));

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let notified = supervisor.notify.notified();
                if supervisor.with_state(|state| state.finalizer_handles.is_empty()) {
                    break;
                }
                notified.await;
            }
        })
        .await
        .unwrap();
        supervisor
            .shutdown_checked(Some(Duration::from_millis(100)))
            .await
            .unwrap();
        assert!(!supervisor.has_active_tasks());
        assert!(supervisor.with_state(|state| {
            state.finalizer_handles.is_empty() && state.completed_finalizers.is_empty()
        }));
    }

    #[tokio::test]
    async fn cancelled_shutdown_reinserts_a_finalizer_held_by_the_drain() {
        let supervisor = Arc::new(BackgroundSubagentSupervisor::new());
        let attempt = SubagentAttemptId::from_string("subattempt_cancelled_shutdown_drain");
        accept(
            &supervisor,
            &attempt,
            "child-bg-cancelled-shutdown-drain",
            false,
        )
        .unwrap();
        let terminal_visible = Arc::new(tokio::sync::Notify::new());
        let release_tail = Arc::new(tokio::sync::Notify::new());
        let finalizing = supervisor.clone();
        let finalizing_attempt = attempt.clone();
        let finalizer_terminal_visible = terminal_visible.clone();
        let finalizer_release_tail = release_tail.clone();
        supervisor
            .spawn_finalizer(attempt.clone(), async move {
                finalizing.record_terminal(
                    &finalizing_attempt,
                    BackgroundSubagentExecutionStatus::Completed,
                    Some("done".to_string()),
                    None,
                );
                finalizer_terminal_visible.notify_one();
                finalizer_release_tail.notified().await;
            })
            .unwrap();
        terminal_visible.notified().await;

        let shutting_down = supervisor.clone();
        let shutdown = tokio::spawn(async move {
            shutting_down
                .shutdown_checked(Some(Duration::from_secs(1)))
                .await
        });
        tokio::time::timeout(Duration::from_secs(1), async {
            while supervisor.with_state(|state| state.finalizer_handles.contains_key(&attempt)) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        shutdown.abort();
        assert!(shutdown.await.unwrap_err().is_cancelled());
        assert!(supervisor.with_state(|state| state.finalizer_handles.contains_key(&attempt)));

        let retrying = supervisor.clone();
        let mut retry = tokio::spawn(async move {
            retrying
                .shutdown_checked(Some(Duration::from_secs(1)))
                .await
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut retry)
                .await
                .is_err()
        );
        release_tail.notify_one();
        retry.await.unwrap().unwrap();
        assert!(supervisor.with_state(|state| state.finalizer_handles.is_empty()));
    }

    #[tokio::test]
    async fn concurrent_shutdowns_serialize_while_a_finalizer_is_being_drained() {
        let supervisor = Arc::new(BackgroundSubagentSupervisor::new());
        let attempt = SubagentAttemptId::from_string("subattempt_concurrent_shutdown_drain");
        accept(
            &supervisor,
            &attempt,
            "child-bg-concurrent-shutdown-drain",
            false,
        )
        .unwrap();
        let terminal_visible = Arc::new(tokio::sync::Notify::new());
        let release_tail = Arc::new(tokio::sync::Notify::new());
        let finalizing = supervisor.clone();
        let finalizing_attempt = attempt.clone();
        let finalizer_terminal_visible = terminal_visible.clone();
        let finalizer_release_tail = release_tail.clone();
        supervisor
            .spawn_finalizer(attempt.clone(), async move {
                finalizing.record_terminal(
                    &finalizing_attempt,
                    BackgroundSubagentExecutionStatus::Completed,
                    Some("done".to_string()),
                    None,
                );
                finalizer_terminal_visible.notify_one();
                finalizer_release_tail.notified().await;
            })
            .unwrap();
        terminal_visible.notified().await;

        let first_supervisor = supervisor.clone();
        let first = tokio::spawn(async move {
            first_supervisor
                .shutdown_checked(Some(Duration::from_secs(1)))
                .await
        });
        tokio::time::timeout(Duration::from_secs(1), async {
            while supervisor.with_state(|state| state.finalizer_handles.contains_key(&attempt)) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let second_supervisor = supervisor.clone();
        let mut second = tokio::spawn(async move {
            second_supervisor
                .shutdown_checked(Some(Duration::from_secs(1)))
                .await
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut second)
                .await
                .is_err()
        );

        release_tail.notify_one();
        first.await.unwrap().unwrap();
        second.await.unwrap().unwrap();
        assert!(supervisor.with_state(|state| state.finalizer_handles.is_empty()));
    }

    #[tokio::test]
    async fn checked_shutdown_joins_finalizer_tail_after_terminal_state_is_visible() {
        let supervisor = Arc::new(BackgroundSubagentSupervisor::new());
        let attempt = SubagentAttemptId::from_string("subattempt_shutdown_finalizer");
        accept(&supervisor, &attempt, "child-bg-shutdown-finalizer", false).unwrap();
        let terminal_visible = Arc::new(tokio::sync::Notify::new());
        let release_tail = Arc::new(tokio::sync::Notify::new());
        let finalizing = supervisor.clone();
        let finalizing_attempt = attempt.clone();
        let finalizer_terminal_visible = terminal_visible.clone();
        let finalizer_release_tail = release_tail.clone();
        supervisor
            .spawn_finalizer(attempt.clone(), async move {
                finalizing.record_terminal(
                    &finalizing_attempt,
                    BackgroundSubagentExecutionStatus::Completed,
                    Some("done".to_string()),
                    None,
                );
                finalizer_terminal_visible.notify_one();
                finalizer_release_tail.notified().await;
            })
            .unwrap();
        terminal_visible.notified().await;
        assert!(supervisor.with_state(|state| state.finalizer_handles.contains_key(&attempt)));

        let shutting_down = supervisor.clone();
        let mut shutdown = tokio::spawn(async move {
            shutting_down
                .shutdown_checked(Some(Duration::from_secs(1)))
                .await
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut shutdown)
                .await
                .is_err()
        );
        release_tail.notify_one();
        shutdown.await.unwrap().unwrap();

        assert!(!supervisor.has_active_tasks());
        assert!(supervisor.with_state(|state| state.finalizer_handles.is_empty()));
        assert_eq!(
            supervisor.task_result(&attempt).unwrap().status,
            BackgroundSubagentExecutionStatus::Completed
        );
    }

    #[test]
    fn expired_artifact_projection_does_not_consume_aggregate_quota() {
        fn artifact_record(
            attempt_id: &str,
            agent_id: &str,
            content: &str,
            expires_at: DateTime<Utc>,
        ) -> BackgroundSubagentRecord {
            let accepted_at = Utc::now() - chrono::Duration::minutes(1);
            let info = BackgroundSubagentTaskInfo {
                attempt_id: SubagentAttemptId::from_string(attempt_id),
                agent_id: agent_id.to_string(),
                subagent_name: "child".to_string(),
                linked_task_id: None,
                execution_status: BackgroundSubagentExecutionStatus::Accepted,
                parent_session_id: Some(SessionId::from_string(format!("session-{attempt_id}"))),
                parent_run_id: Some(RunId::from_string(format!("run-{attempt_id}"))),
                child_run_id: None,
                is_resume: false,
                prompt_preview: "task".to_string(),
                accepted_at,
                updated_at: accepted_at,
            };
            let mut record = durable_acceptance_record(
                &info,
                "test",
                "test-profile".to_string(),
                TraceContext::default(),
                "test-host",
                1,
                Duration::from_secs(30),
            )
            .unwrap();
            let digest = BackgroundSubagentArtifact::content_digest(content);
            record.execution_status = DurableBackgroundSubagentExecutionStatus::Completed;
            record.result_ref = Some(DurableBackgroundSubagentResultRef {
                content: None,
                error: None,
                artifact_ref: Some(format!("artifact://{attempt_id}")),
                digest: Some(digest),
                size_bytes: u64::try_from(content.len()).unwrap(),
            });
            record.retention_status = DurableBackgroundSubagentRetentionStatus::Artifact;
            record.retention_expires_at = Some(expires_at);
            record.terminal_at = Some(accepted_at);
            record
        }

        let supervisor = BackgroundSubagentSupervisor::with_limits(BackgroundSubagentLimits {
            max_artifact_result_bytes: 6,
            max_retained_artifact_bytes: 6,
            ..BackgroundSubagentLimits::default()
        });
        let expired = artifact_record(
            "subattempt_expired_artifact",
            "child-bg-expired-artifact",
            "first!",
            Utc::now() - chrono::Duration::seconds(1),
        );
        assert!(
            supervisor
                .hydrate_durable_result(&expired, Some("first!".to_string()))
                .unwrap()
        );
        let current = artifact_record(
            "subattempt_current_artifact",
            "child-bg-current-artifact",
            "second",
            Utc::now() + chrono::Duration::minutes(1),
        );
        assert!(
            supervisor
                .hydrate_durable_result(&current, Some("second".to_string()))
                .unwrap()
        );
        assert_eq!(supervisor.task_results().len(), 2);
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
        assert_eq!(
            supervisor.spawn_finalizer(rejected, async {}).unwrap_err(),
            BackgroundSubagentError::Closing
        );
    }
}
