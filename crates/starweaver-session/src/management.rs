//! Product-neutral agent-facing session-management contracts.
//!
//! These types deliberately contain no product services or transport details. A host constructs
//! [`AgentSessionScope`] and implements the narrow handles in `starweaver-agent`.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use starweaver_core::{RunId, SessionId};
use starweaver_stream::{DisplayMessage, ReplayCursor};

use crate::{InputPart, RunStatus, SessionStatus};

/// Backward-compatible namespace used by single-user local products.
pub const LOCAL_SESSION_NAMESPACE: &str = "local";

/// Composite identity of a durable session.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct ManagedSessionTarget {
    /// Host-derived tenant/store namespace.
    pub namespace_id: String,
    /// Durable session id within the namespace.
    pub session_id: SessionId,
}

impl ManagedSessionTarget {
    /// Build a composite session target.
    #[must_use]
    pub fn new(namespace_id: impl Into<String>, session_id: SessionId) -> Self {
        Self {
            namespace_id: namespace_id.into(),
            session_id,
        }
    }
}

/// Composite identity of a durable run. A run id is never globally unique.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct ManagedRunTarget {
    /// Host-derived tenant/store namespace.
    pub namespace_id: String,
    /// Owning session id.
    pub session_id: SessionId,
    /// Session-scoped run id.
    pub run_id: RunId,
}

impl ManagedRunTarget {
    /// Build a composite run target.
    #[must_use]
    pub fn new(namespace_id: impl Into<String>, session_id: SessionId, run_id: RunId) -> Self {
        Self {
            namespace_id: namespace_id.into(),
            session_id,
            run_id,
        }
    }
}

/// Agent-facing session operation class.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentSessionOperation {
    /// List/get session and run projections.
    Read,
    /// Use an independently injected search provider.
    Search,
    /// Create a session or start a run.
    Create,
    /// Update title/profile/archive state.
    Update,
    /// Steer or interrupt a live run.
    Control,
    /// Tombstone a session.
    Delete,
}

/// Host-derived authority supplied separately from model arguments.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentSessionScope {
    /// Authorized namespace.
    pub namespace_id: String,
    /// Principal/owner represented by this capability.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_id: Option<String>,
    /// Product that constructed the scope.
    pub source_product: String,
    /// Controlling session, used for self-target denial.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_session_id: Option<SessionId>,
    /// Controlling run, used for self-target denial.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_run_id: Option<RunId>,
    /// Effective operation intersection. Model input cannot alter this set.
    #[serde(default)]
    pub operations: BTreeSet<AgentSessionOperation>,
    /// Optional allowlist; empty means the host's namespace/owner policy decides.
    #[serde(default)]
    pub allowed_session_ids: BTreeSet<SessionId>,
    /// Whether the current session may be queried.
    #[serde(default = "default_true")]
    pub allow_self_query: bool,
    /// Whether the current run/session may be controlled. Hosts should normally leave false.
    #[serde(default)]
    pub allow_self_control: bool,
    /// Stable fingerprint of the intersected server/profile/caller/grant policy.
    pub policy_fingerprint: String,
    /// Absolute deadline for commands made through this capability.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline: Option<DateTime<Utc>>,
    /// Maximum page size after host policy intersection.
    #[serde(default = "default_page_limit")]
    pub max_page_size: u32,
}

const fn default_true() -> bool {
    true
}

const fn default_page_limit() -> u32 {
    50
}

impl AgentSessionScope {
    /// Return whether an operation is in the effective grant.
    #[must_use]
    pub fn allows(&self, operation: AgentSessionOperation) -> bool {
        self.operations.contains(&operation)
    }

    /// Return whether a session id is inside the host allowlist.
    #[must_use]
    pub fn allows_session(&self, session_id: &SessionId) -> bool {
        self.allowed_session_ids.is_empty() || self.allowed_session_ids.contains(session_id)
    }

    /// Return whether a run target is the controlling run.
    #[must_use]
    pub fn is_self_run(&self, target: &ManagedRunTarget) -> bool {
        self.namespace_id == target.namespace_id
            && self.source_session_id.as_ref() == Some(&target.session_id)
            && self.source_run_id.as_ref() == Some(&target.run_id)
    }

    /// Return whether a session target is the controlling session.
    #[must_use]
    pub fn is_self_session(&self, target: &ManagedSessionTarget) -> bool {
        self.namespace_id == target.namespace_id
            && self.source_session_id.as_ref() == Some(&target.session_id)
    }
}

/// Explicit session deletion state/fence used by run and continuation admission.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SessionDeletionFence {
    /// No deletion is in progress.
    #[default]
    Stable,
    /// New runs, continuations, and child delegation are fenced.
    Deleting {
        /// Stable deletion intent id.
        fence_id: String,
        /// Revision from which deletion was acquired.
        expected_revision: u64,
        /// Principal requesting deletion.
        requested_by: String,
        /// Fence acquisition time.
        started_at: DateTime<Utc>,
    },
    /// Session is tombstoned; evidence retention is an admin concern.
    Deleted {
        /// Stable deletion intent id.
        fence_id: String,
        /// Tombstone time.
        deleted_at: DateTime<Utc>,
    },
}

impl SessionDeletionFence {
    /// Return whether new work and async continuations must be denied.
    #[must_use]
    pub const fn blocks_continuation(&self) -> bool {
        !matches!(self, Self::Stable)
    }
}

/// Deletion/continuation hook exposed to async supervisors without coupling both control planes.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SessionContinuationFence {
    /// Composite session target.
    pub target: ManagedSessionTarget,
    /// Session revision at the authorization check.
    pub revision: u64,
    /// True when continuation/delegation is allowed.
    pub continuation_allowed: bool,
    /// Stable deletion fence id when blocked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fence_id: Option<String>,
}

/// Compact query for canonical session listing.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentSessionListQuery {
    /// Optional exact status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<SessionStatus>,
    /// Optional exact profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// Optional exact workspace display value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    /// Maximum page size.
    #[serde(default = "default_page_limit")]
    pub limit: u32,
    /// Opaque keyset token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_token: Option<String>,
}

/// Sections requested by a get-session operation.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentSessionInclude {
    /// Include bounded recent run summaries.
    #[serde(default)]
    pub recent_runs: bool,
    /// Include compact trace counts.
    #[serde(default)]
    pub trace: bool,
}

/// Compact run list query.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentRunListQuery {
    /// Maximum page size.
    #[serde(default = "default_page_limit")]
    pub limit: u32,
    /// Opaque session-local keyset token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_token: Option<String>,
}

impl Default for AgentRunListQuery {
    fn default() -> Self {
        Self {
            limit: default_page_limit(),
            page_token: None,
        }
    }
}

/// Display-safe replay query.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentReplayQuery {
    /// Family-aware cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<ReplayCursor>,
    /// Maximum number of display messages.
    #[serde(default = "default_page_limit")]
    pub limit: u32,
}

/// Prompt-safe compact session projection.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentSessionView {
    /// Composite target.
    pub target: ManagedSessionTarget,
    /// Optional bounded title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Canonical status.
    pub status: SessionStatus,
    /// Optional profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// Safe workspace display value, never a backend locator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    /// Optimistic concurrency token.
    pub revision: u64,
    /// Head run id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_run_id: Option<RunId>,
    /// Current active run id when canonical storage has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_run_id: Option<RunId>,
    /// Whether the current host can resume this session.
    pub resumable: bool,
    /// Whether the current host has a fenced local control handle.
    pub controllable: bool,
    /// Bounded recent run summaries.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_runs: Vec<AgentRunView>,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Last update time.
    pub updated_at: DateTime<Utc>,
}

/// Prompt-safe compact run projection.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentRunView {
    /// Composite run target.
    pub target: ManagedRunTarget,
    /// Durable status.
    pub status: RunStatus,
    /// Session-local order.
    pub sequence_no: usize,
    /// Bounded text input preview.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_preview: Option<String>,
    /// Bounded output preview.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_preview: Option<String>,
    /// Safe error category, never raw provider diagnostics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_category: Option<String>,
    /// Whether this process currently owns fenced control.
    pub controllable: bool,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Last update time.
    pub updated_at: DateTime<Utc>,
}

/// Page of compact sessions.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentSessionPage {
    /// Results.
    pub sessions: Vec<AgentSessionView>,
    /// Opaque continuation token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_page_token: Option<String>,
}

/// Page of compact runs.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentRunPage {
    /// Results.
    pub runs: Vec<AgentRunView>,
    /// Opaque continuation token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_page_token: Option<String>,
}

/// Page of sanitized historical display evidence.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentDisplayPage {
    /// Display-safe messages. Hosts must exclude internal and diagnostic visibility.
    pub messages: Vec<DisplayMessage>,
    /// Cursor after the final returned message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<ReplayCursor>,
    /// Explicit reminder that historical content is untrusted evidence.
    #[serde(default = "untrusted_evidence_label")]
    pub trust: String,
}

fn untrusted_evidence_label() -> String {
    "untrusted_historical_evidence".to_string()
}

/// Allowlisted create-session command.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CreateManagedSession {
    /// Optional title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Optional configured profile id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// Host-approved workspace reference/display value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    /// Approved typed metadata only.
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
    /// Stable idempotency key.
    pub idempotency_key: String,
}

/// Typed session update patch.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManagedSessionPatch {
    /// Set or clear the title. Outer `None` means unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<Option<String>>,
    /// Set or clear the future-run profile. Outer `None` means unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<Option<String>>,
    /// Explicit archive transition.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
    /// Allowlisted metadata replacements/removals (`null` removes).
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
}

/// Revision-checked session update.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct UpdateManagedSession {
    /// Session id; namespace comes from scope.
    pub session_id: SessionId,
    /// Required optimistic revision.
    pub expected_revision: u64,
    /// Explicit patch.
    pub patch: ManagedSessionPatch,
    /// Stable idempotency key.
    pub idempotency_key: String,
}

/// Revision-checked tombstone command. Evidence purge is intentionally absent.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DeleteManagedSession {
    /// Session id; namespace comes from scope.
    pub session_id: SessionId,
    /// Required optimistic revision.
    pub expected_revision: u64,
    /// Stable idempotency key.
    pub idempotency_key: String,
    /// Approval evidence supplied by the host approval layer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_receipt_id: Option<String>,
}

/// Non-blocking run start command.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StartManagedRun {
    /// Owning session id.
    pub session_id: SessionId,
    /// Canonical model input.
    pub input: Vec<InputPart>,
    /// Optional configured profile override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// Approved environment attachment references.
    #[serde(default)]
    pub environment_refs: Vec<String>,
    /// Stable idempotency key.
    pub idempotency_key: String,
}

/// Structured steering command.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SteerManagedRun {
    /// Composite target.
    pub target: ManagedRunTarget,
    /// Stable steering id.
    pub steering_id: String,
    /// Bounded text baseline.
    pub text: String,
    /// Optional idempotency key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
}

/// Cooperative interruption command.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct InterruptManagedRun {
    /// Composite target.
    pub target: ManagedRunTarget,
    /// Stable control operation id.
    pub operation_id: String,
    /// Safe bounded reason category.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_category: Option<String>,
    /// Optional idempotency key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
}

/// Session mutation result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SessionMutationReceipt {
    /// Stable receipt id.
    pub receipt_id: String,
    /// Compact canonical projection after mutation.
    pub session: AgentSessionView,
    /// True when an exact idempotent result was replayed.
    pub idempotent_replay: bool,
}

/// Accepted non-blocking run result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunStartReceipt {
    /// Stable receipt id.
    pub receipt_id: String,
    /// Composite target and fencing generation.
    pub target: ManagedRunTarget,
    /// Accepted durable status.
    pub status: RunStatus,
    /// Admission fencing generation.
    pub fencing_generation: u64,
    /// True when an exact idempotent result was replayed.
    pub idempotent_replay: bool,
}

/// Accepted steering/interruption result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunControlReceipt {
    /// Stable durable receipt id.
    pub receipt_id: String,
    /// Composite target.
    pub target: ManagedRunTarget,
    /// Operation id or steering id.
    pub operation_id: String,
    /// Fencing generation against which it was accepted.
    pub fencing_generation: u64,
    /// Accepted means queued/signalled, not completed/consumed.
    pub accepted: bool,
    /// True when an exact idempotent result was replayed.
    pub idempotent_replay: bool,
    /// Receipt creation time.
    pub created_at: DateTime<Utc>,
}

/// Durable one-active-run lease owned by a host instance.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunAdmissionLease {
    /// Composite target.
    pub target: ManagedRunTarget,
    /// Stable admission intent id.
    pub admission_id: String,
    /// Current host instance id.
    pub host_instance_id: String,
    /// Monotonic fencing generation for this session.
    pub fencing_generation: u64,
    /// Lease expiry.
    pub lease_expires_at: DateTime<Utc>,
    /// Last heartbeat.
    pub heartbeat_at: DateTime<Utc>,
    /// Normalized start-command fingerprint.
    pub command_fingerprint: String,
    /// Idempotency key bound to the fingerprint.
    pub idempotency_key: String,
}

impl starweaver_core::VersionedRecord for RunAdmissionLease {
    const SCHEMA: &'static str = "starweaver.session.run_admission_lease";
}

impl RunAdmissionLease {
    /// Return whether this lease is expired at `now`.
    #[must_use]
    pub fn expired_at(&self, now: DateTime<Utc>) -> bool {
        self.lease_expires_at <= now
    }
}

/// Request for atomic one-active-run admission.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AcquireRunAdmission {
    /// Queued/starting run record to persist atomically with the lease.
    pub run: crate::RunRecord,
    /// Namespace derived by the product.
    pub namespace_id: String,
    /// Host instance claiming ownership.
    pub host_instance_id: String,
    /// Stable admission id.
    pub admission_id: String,
    /// Lease expiry.
    pub lease_expires_at: DateTime<Utc>,
    /// Stable idempotency key.
    pub idempotency_key: String,
    /// Normalized command fingerprint.
    pub command_fingerprint: String,
}

/// Result of admission or an exact retry.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunAdmissionReceipt {
    /// Persisted run.
    pub run: crate::RunRecord,
    /// Durable lease/fencing evidence.
    pub lease: RunAdmissionLease,
    /// True for same-key/same-command retry.
    pub idempotent_replay: bool,
}

impl starweaver_core::VersionedRecord for RunAdmissionReceipt {
    const SCHEMA: &'static str = "starweaver.session.run_admission_receipt";
}

/// Durable receipt stored independently from a process-local control handle.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DurableControlReceipt {
    /// Stable receipt id.
    pub receipt_id: String,
    /// Composite target.
    pub target: ManagedRunTarget,
    /// Steering/interruption operation id.
    pub operation_id: String,
    /// Operation category.
    pub operation: String,
    /// Idempotency key.
    pub idempotency_key: String,
    /// Safe normalized fingerprint.
    pub command_fingerprint: String,
    /// Matching owner generation.
    pub fencing_generation: u64,
    /// Effect state (`reserved`, `accepted`, or `failed`).
    pub state: String,
    /// Creation time.
    pub created_at: DateTime<Utc>,
}

impl starweaver_core::VersionedRecord for DurableControlReceipt {
    const SCHEMA: &'static str = "starweaver.session.durable_control_receipt";
}

/// Required query error categories.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentSessionQueryErrorCode {
    /// Query fields or limits are invalid.
    InvalidQuery,
    /// Authorized canonical target was not found (also used to hide unauthorized targets).
    NotFound,
    /// Optional capability is not installed.
    Unsupported,
    /// Canonical provider is temporarily unavailable.
    Unavailable,
    /// Effective host scope denies the operation.
    PermissionDenied,
    /// Cursor is malformed, stale, or belongs to another query/scope.
    InvalidCursor,
    /// Bounded internal query failure.
    Failed,
}

/// Prompt-safe query error.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentSessionQueryError {
    /// Stable category.
    pub code: AgentSessionQueryErrorCode,
    /// Bounded safe message.
    pub message: String,
}

impl std::fmt::Display for AgentSessionQueryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{:?}: {}", self.code, self.message)
    }
}

impl std::error::Error for AgentSessionQueryError {}

/// Required control error categories.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentSessionControlErrorCode {
    /// Command fields or limits are invalid.
    InvalidCommand,
    /// Authorized target was not found (also used to hide unauthorized targets).
    NotFound,
    /// Effective host scope denies the operation.
    PermissionDenied,
    /// A host approval receipt is required.
    ApprovalRequired,
    /// Optimistic revision or lifecycle conflict.
    Conflict,
    /// Idempotency key was reused for a different normalized command.
    IdempotencyConflict,
    /// Session already owns an active run slot.
    RunConflict,
    /// Target has no current process-local control handle.
    NotActive,
    /// Target run is immutable and terminal.
    Terminal,
    /// Durable state is active but no matching fenced owner exists.
    StaleActive,
    /// Host quota rejects the operation.
    QuotaExceeded,
    /// Required coordinator or storage capability is unavailable.
    Unavailable,
    /// Bounded internal control failure.
    Failed,
}

/// Prompt-safe control error.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentSessionControlError {
    /// Stable category.
    pub code: AgentSessionControlErrorCode,
    /// Bounded safe message.
    pub message: String,
    /// Current safe session revision for CAS conflicts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_revision: Option<u64>,
}

impl std::fmt::Display for AgentSessionControlError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{:?}: {}", self.code, self.message)
    }
}

impl std::error::Error for AgentSessionControlError {}
