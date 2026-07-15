//! Product-neutral durable background-subagent execution and delivery records.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use starweaver_core::{RunId, SessionId, SubagentAttemptId, TaskId, TraceContext};

use crate::{AcquireRunAdmission, InputPart, RunAdmissionReceipt, RunRecord, RunStatus};

/// Current durable background-subagent record schema.
pub const BACKGROUND_SUBAGENT_RECORD_VERSION: u32 = 1;
/// Default retained-result lifetime used by store-side interruption reconciliation.
pub const DEFAULT_BACKGROUND_RESULT_RETENTION_SECS: i64 = 86_400;

/// Durable host-owned oversized result artifact.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BackgroundSubagentArtifact {
    /// Stable artifact reference stored in the terminal result record.
    pub artifact_ref: String,
    /// Owning durable namespace.
    pub namespace_id: String,
    /// Attempt whose successful output was externalized.
    pub attempt_id: SubagentAttemptId,
    /// Complete successful output bytes encoded as UTF-8 text.
    pub content: String,
    /// SHA-256 digest of the complete logical result.
    pub digest: String,
    /// Complete logical result size in bytes.
    pub size_bytes: u64,
    /// Artifact creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Policy-controlled retention deadline.
    pub expires_at: DateTime<Utc>,
}

impl starweaver_core::VersionedRecord for BackgroundSubagentArtifact {
    const SCHEMA: &'static str = "starweaver.session.background_subagent_artifact";
    const VERSION: u32 = 1;
}

impl BackgroundSubagentArtifact {
    /// Compute the domain-separated digest used for successful result content.
    #[must_use]
    pub fn content_digest(content: &str) -> String {
        background_subagent_result_digest(Some(content), None)
    }

    /// Return whether the artifact matches its durable record identity and content.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        !self.artifact_ref.is_empty()
            && !self.namespace_id.is_empty()
            && self.digest == Self::content_digest(&self.content)
            && self.size_bytes == u64::try_from(self.content.len()).unwrap_or(u64::MAX)
            && self.expires_at > self.created_at
    }

    /// Return whether the artifact is valid and still retained at `now`.
    #[must_use]
    pub fn is_available_at(&self, now: DateTime<Utc>) -> bool {
        self.is_valid() && self.expires_at > now
    }
}

/// Host policy limits applied atomically to retained result artifacts.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BackgroundSubagentArtifactLimits {
    /// Maximum bytes accepted for one artifact.
    pub max_single_bytes: u64,
    /// Maximum aggregate unexpired artifact bytes in one durable namespace.
    pub max_retained_bytes: u64,
}

impl BackgroundSubagentArtifactLimits {
    /// Return whether the configured limits are non-zero and internally ordered.
    #[must_use]
    pub const fn is_valid(self) -> bool {
        self.max_single_bytes > 0
            && self.max_retained_bytes > 0
            && self.max_single_bytes <= self.max_retained_bytes
    }
}

/// Atomic terminal evidence and optional oversized-result artifact commit.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BackgroundSubagentTerminalCommit {
    /// Immutable terminal execution and delivery record.
    pub record: BackgroundSubagentRecord,
    /// Optional complete successful output externalized from the bounded preview.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact: Option<BackgroundSubagentArtifact>,
    /// Required host quota policy whenever an artifact payload is supplied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_limits: Option<BackgroundSubagentArtifactLimits>,
}

#[derive(Serialize)]
struct CanonicalBackgroundSubagentTerminalCommit<'a> {
    schema_version: u32,
    attempt_id: &'a SubagentAttemptId,
    agent_id: &'a str,
    linked_task_id: &'a Option<TaskId>,
    subagent_name: &'a str,
    namespace_id: &'a str,
    parent_session_id: &'a SessionId,
    parent_run_id: &'a RunId,
    child_run_id: &'a Option<RunId>,
    profile: &'a str,
    owner_host_instance_id: &'a str,
    owner_fencing_generation: u64,
    execution_status: DurableBackgroundSubagentExecutionStatus,
    result_ref: &'a Option<DurableBackgroundSubagentResultRef>,
    failure_category: &'a Option<String>,
    cancellation_reason: &'a Option<String>,
    initial_retention_status: DurableBackgroundSubagentRetentionStatus,
    initial_retention_expires_at: &'a Option<DateTime<Utc>>,
    trace_context: &'a Option<TraceContext>,
    accepted_at: &'a DateTime<Utc>,
    terminal_at: &'a Option<DateTime<Utc>>,
    updated_at: &'a DateTime<Utc>,
    artifact: &'a Option<BackgroundSubagentArtifact>,
}

impl BackgroundSubagentTerminalCommit {
    /// Compute the canonical fingerprint persisted for exact terminal retries.
    ///
    /// Logical delivery fields and their later projections are deliberately excluded. Initial
    /// retention evidence and the complete artifact payload remain covered even after content is
    /// expired from the live record.
    ///
    /// # Errors
    ///
    /// Returns a serialization error when the canonical evidence cannot be encoded.
    pub fn canonical_fingerprint(&self) -> Result<String, serde_json::Error> {
        let record = &self.record;
        let canonical = CanonicalBackgroundSubagentTerminalCommit {
            schema_version: record.schema_version,
            attempt_id: &record.attempt_id,
            agent_id: &record.agent_id,
            linked_task_id: &record.linked_task_id,
            subagent_name: &record.subagent_name,
            namespace_id: &record.namespace_id,
            parent_session_id: &record.parent_session_id,
            parent_run_id: &record.parent_run_id,
            child_run_id: &record.child_run_id,
            profile: &record.profile,
            owner_host_instance_id: &record.owner_lease.host_instance_id,
            owner_fencing_generation: record.owner_lease.fencing_generation,
            execution_status: record.execution_status,
            result_ref: &record.result_ref,
            failure_category: &record.failure_category,
            cancellation_reason: &record.cancellation_reason,
            initial_retention_status: record.retention_status,
            initial_retention_expires_at: &record.retention_expires_at,
            trace_context: &record.trace_context,
            accepted_at: &record.accepted_at,
            terminal_at: &record.terminal_at,
            updated_at: &record.updated_at,
            artifact: &self.artifact,
        };
        let payload = serde_json::to_vec(&canonical)?;
        let mut hasher = Sha256::new();
        hasher.update(b"starweaver.session.background_subagent.terminal_commit.v1\0");
        hasher.update(payload);
        Ok(format!("sha256:{:x}", hasher.finalize()))
    }
}

/// Monotonic durable execution state for one background attempt.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DurableBackgroundSubagentExecutionStatus {
    /// Identity, quota, ownership, and durable admission exist.
    Accepted,
    /// Child construction is in progress.
    Starting,
    /// Child execution can make progress.
    Running,
    /// The same attempt is waiting for an explicitly resumable condition.
    Waiting,
    /// Child execution completed successfully.
    Completed,
    /// Child execution failed or was interrupted after process loss.
    Failed,
    /// Child execution was cancelled.
    Cancelled,
}

impl DurableBackgroundSubagentExecutionStatus {
    /// Return whether this is an immutable terminal outcome.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }

    /// Return the stable wire-format status name.
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
}

/// Orthogonal durable logical-delivery state.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DurableBackgroundSubagentDeliveryStatus {
    /// Terminal content exists but no consumer owns it.
    #[default]
    Undelivered,
    /// One bounded consumer owns delivery.
    Claimed,
    /// A parent turn, explicit wait, or admitted continuation consumed it.
    Delivered,
}

/// Durable content-retention state, independent from execution and delivery.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DurableBackgroundSubagentRetentionStatus {
    /// Bounded terminal content is stored inline.
    #[default]
    Inline,
    /// Content is represented by a host-owned artifact reference.
    Artifact,
    /// Volatile content expired while minimal audit evidence remains.
    Expired,
}

impl DurableBackgroundSubagentRetentionStatus {
    /// Stable storage and protocol name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Inline => "inline",
            Self::Artifact => "artifact",
            Self::Expired => "expired",
        }
    }
}

/// Bounded terminal result material retained by the durable host.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct DurableBackgroundSubagentResultRef {
    /// Bounded successful text content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Bounded safe error text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Optional host artifact locator for oversized content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_ref: Option<String>,
    /// Stable digest of the retained logical content when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    /// Logical content size before host retention policy was applied.
    #[serde(default)]
    pub size_bytes: u64,
}

/// Atomic durable delivery claim.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DurableBackgroundSubagentDeliveryClaim {
    /// Stable consumer-generated claim identity.
    pub claim_id: String,
    /// Optional parent continuation admitted under this claim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation_run_id: Option<RunId>,
    /// Absolute claim expiry.
    pub deadline: DateTime<Utc>,
}

/// Durable reason for releasing a matching result-delivery claim.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DurableBackgroundSubagentDeliveryRelease {
    /// Admission or another retryable pre-consumption step failed.
    #[default]
    Retryable,
    /// A parent run claimed the result but failed or was cancelled before committing it.
    ConsumerTerminated {
        /// Failed or cancelled run that owned the released claim.
        run_id: RunId,
    },
}

/// Durable fenced owner lease for one process-local background execution.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DurableBackgroundSubagentOwnerLease {
    /// Service instance currently authorized to advance this attempt.
    pub host_instance_id: String,
    /// Monotonic fencing generation for ownership-sensitive writes.
    pub fencing_generation: u64,
    /// Last successful owner heartbeat.
    pub heartbeat_at: DateTime<Utc>,
    /// Time after which reconciliation may classify the process-local execution as lost.
    pub lease_expires_at: DateTime<Utc>,
}

impl DurableBackgroundSubagentOwnerLease {
    /// Return whether this owner lease is structurally valid.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        !self.host_instance_id.is_empty()
            && self.fencing_generation > 0
            && self.lease_expires_at > self.heartbeat_at
    }

    /// Return whether the lease is expired at `now`.
    #[must_use]
    pub fn expired_at(&self, now: DateTime<Utc>) -> bool {
        self.lease_expires_at <= now
    }
}

/// Durable service-host projection of one background attempt.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BackgroundSubagentRecord {
    /// Version of this serialized record contract.
    pub schema_version: u32,
    /// Per-execution attempt identity.
    pub attempt_id: SubagentAttemptId,
    /// Stable child conversation identity within the owning supervisor scope.
    pub agent_id: String,
    /// Optional independently owned task-bundle identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked_task_id: Option<TaskId>,
    /// Configured subagent name.
    pub subagent_name: String,
    /// Durable host namespace.
    pub namespace_id: String,
    /// Owning parent session.
    pub parent_session_id: SessionId,
    /// Parent run that accepted delegation.
    pub parent_run_id: RunId,
    /// Child runtime run once known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_run_id: Option<RunId>,
    /// Parent continuation that consumed the result, when any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation_run_id: Option<RunId>,
    /// Resolved parent profile used for an automatic continuation.
    pub profile: String,
    /// Fenced process owner for all non-terminal execution writes.
    pub owner_lease: DurableBackgroundSubagentOwnerLease,
    /// Current monotonic execution state.
    pub execution_status: DurableBackgroundSubagentExecutionStatus,
    /// Terminal result reference, separate from minimal lifecycle evidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_ref: Option<DurableBackgroundSubagentResultRef>,
    /// Safe failure/interruption category.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_category: Option<String>,
    /// Bounded cancellation reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancellation_reason: Option<String>,
    /// Logical result-delivery state.
    pub delivery_status: DurableBackgroundSubagentDeliveryStatus,
    /// Current bounded claim, when delivery is claimed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery_claim: Option<DurableBackgroundSubagentDeliveryClaim>,
    /// Claim that completed logical delivery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivered_claim_id: Option<String>,
    /// Failed or cancelled consumer that suppresses automatic continuation redelivery.
    ///
    /// Explicit later parent runs may still claim and consume the pending result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub automatic_continuation_suppressed_by_run_id: Option<RunId>,
    /// Independent content-retention state.
    pub retention_status: DurableBackgroundSubagentRetentionStatus,
    /// Policy deadline for retained inline or artifact content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention_expires_at: Option<DateTime<Utc>>,
    /// Parent trace correlation copied at acceptance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_context: Option<TraceContext>,
    /// Acceptance time.
    pub accepted_at: DateTime<Utc>,
    /// Last durable transition time.
    pub updated_at: DateTime<Utc>,
    /// Immutable terminal transition time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_at: Option<DateTime<Utc>>,
}

impl starweaver_core::VersionedRecord for BackgroundSubagentRecord {
    const SCHEMA: &'static str = "starweaver.session.background_subagent_record";
    const VERSION: u32 = BACKGROUND_SUBAGENT_RECORD_VERSION;
}

impl BackgroundSubagentRecord {
    /// Return whether this record is a canonical acceptance projection.
    #[must_use]
    pub fn is_valid_acceptance(&self) -> bool {
        self.execution_status == DurableBackgroundSubagentExecutionStatus::Accepted
            && self.owner_lease.is_valid()
            && self.owner_lease.heartbeat_at == self.accepted_at
            && self.child_run_id.is_none()
            && self.result_ref.is_none()
            && self.failure_category.is_none()
            && self.cancellation_reason.is_none()
            && self.delivery_status == DurableBackgroundSubagentDeliveryStatus::Undelivered
            && self.delivery_claim.is_none()
            && self.delivered_claim_id.is_none()
            && self.continuation_run_id.is_none()
            && self.automatic_continuation_suppressed_by_run_id.is_none()
            && self.retention_status == DurableBackgroundSubagentRetentionStatus::Inline
            && self.retention_expires_at.is_none()
            && self.terminal_at.is_none()
            && self.updated_at == self.accepted_at
    }

    /// Return whether this record is a canonical immutable terminal projection.
    #[must_use]
    pub fn is_valid_terminal(&self) -> bool {
        self.execution_status.is_terminal()
            && self.owner_lease.is_valid()
            && self.result_ref.is_some()
            && self.delivery_status == DurableBackgroundSubagentDeliveryStatus::Undelivered
            && self.delivery_claim.is_none()
            && self.delivered_claim_id.is_none()
            && self.continuation_run_id.is_none()
            && self.automatic_continuation_suppressed_by_run_id.is_none()
            && match self.retention_status {
                DurableBackgroundSubagentRetentionStatus::Expired => {
                    self.retention_expires_at.is_none()
                        && self.result_ref.as_ref().is_some_and(|result| {
                            result.content.is_none()
                                && result.error.is_none()
                                && result.artifact_ref.is_none()
                        })
                }
                _ => self
                    .retention_expires_at
                    .is_some_and(|deadline| deadline > self.updated_at),
            }
            && self.terminal_at == Some(self.updated_at)
            && self.updated_at >= self.accepted_at
            && (self.execution_status != DurableBackgroundSubagentExecutionStatus::Completed
                || (self.failure_category.is_none() && self.cancellation_reason.is_none()))
            && (self.cancellation_reason.is_none()
                || self.execution_status == DurableBackgroundSubagentExecutionStatus::Cancelled)
    }

    /// Return whether a claim may be acquired at `now`.
    #[must_use]
    pub fn delivery_claimable_at(&self, now: DateTime<Utc>) -> bool {
        self.execution_status.is_terminal()
            && (self.delivery_status == DurableBackgroundSubagentDeliveryStatus::Undelivered
                || (self.delivery_status == DurableBackgroundSubagentDeliveryStatus::Claimed
                    && self
                        .delivery_claim
                        .as_ref()
                        .is_some_and(|claim| claim.deadline <= now)))
    }

    /// Resolve the canonical model-visible outcome for a continuation.
    #[must_use]
    pub fn continuation_outcome(&self, artifact_content: Option<&str>) -> String {
        if self.retention_status == DurableBackgroundSubagentRetentionStatus::Expired {
            let result_ref = self.result_ref.as_ref();
            return format!(
                "Retained background result content expired (digest: {}, logical_size_bytes: {}).",
                result_ref
                    .and_then(|result| result.digest.as_deref())
                    .unwrap_or("unknown"),
                result_ref.map_or(0, |result| result.size_bytes),
            );
        }
        if let Some(content) = artifact_content {
            return content.to_string();
        }
        self.result_ref.as_ref().map_or_else(
            || {
                format!(
                    "Background subagent finished with status {}.",
                    self.execution_status.as_str()
                )
            },
            |result| {
                result
                    .content
                    .clone()
                    .or_else(|| result.error.clone())
                    .unwrap_or_else(|| {
                        format!(
                            "Background subagent finished with status {}.",
                            self.execution_status.as_str()
                        )
                    })
            },
        )
    }

    /// Build the canonical model-visible continuation text.
    #[must_use]
    pub fn continuation_text(&self, artifact_content: Option<&str>) -> String {
        format!(
            "Background subagent result (subagent: {}, agent_id: {}, attempt_id: {}, child_run_id: {}):\n\n{}",
            self.subagent_name,
            self.agent_id,
            self.attempt_id.as_str(),
            self.child_run_id.as_ref().map_or("unknown", RunId::as_str),
            self.continuation_outcome(artifact_content),
        )
    }

    /// Build the exact durable input accepted for a result-triggered continuation.
    #[must_use]
    pub fn continuation_input(&self, artifact_content: Option<&str>) -> Vec<InputPart> {
        vec![InputPart::text(self.continuation_text(artifact_content))]
    }

    /// Validate immutable causal fields on a proposed result-triggered continuation.
    #[must_use]
    pub fn validates_continuation_run(&self, run: &RunRecord) -> bool {
        run.session_id == self.parent_session_id
            && run.status == RunStatus::Queued
            && run.parent_run_id.as_ref() == Some(&self.parent_run_id)
            && run.trigger_type.as_deref() == Some("async_subagent_result")
            && run.profile.as_deref() == Some(self.profile.as_str())
            && run
                .metadata
                .get("starweaver.async_subagent.attempt_id")
                .and_then(serde_json::Value::as_str)
                == Some(self.attempt_id.as_str())
            && run
                .metadata
                .get("starweaver.async_subagent.agent_id")
                .and_then(serde_json::Value::as_str)
                == Some(self.agent_id.as_str())
            && run
                .metadata
                .get("starweaver.async_subagent.parent_run_id")
                .and_then(serde_json::Value::as_str)
                == Some(self.parent_run_id.as_str())
            && self.child_run_id.as_ref().map_or_else(
                || {
                    !run.metadata
                        .contains_key("starweaver.async_subagent.child_run_id")
                },
                |child_run_id| {
                    run.metadata
                        .get("starweaver.async_subagent.child_run_id")
                        .and_then(serde_json::Value::as_str)
                        == Some(child_run_id.as_str())
                },
            )
            && run.trace_context == self.trace_context.clone().unwrap_or_default()
    }

    /// Validate the typed causal envelope and its submitted input digest.
    #[must_use]
    pub fn validates_continuation_cause_envelope(
        &self,
        cause: &BackgroundSubagentContinuationCause,
        run: &RunRecord,
    ) -> bool {
        let Ok(input_digest) = background_subagent_input_digest(&run.input) else {
            return false;
        };
        let result_ref = self.result_ref.as_ref();
        self.validates_continuation_run(run)
            && cause.attempt_id == self.attempt_id
            && cause.agent_id == self.agent_id
            && cause.parent_session_id == self.parent_session_id
            && cause.parent_run_id == self.parent_run_id
            && cause.child_run_id == self.child_run_id
            && cause.result_digest == result_ref.and_then(|result| result.digest.clone())
            && cause.result_size_bytes == result_ref.map_or(0, |result| result.size_bytes)
            && cause.trace_context == self.trace_context.clone().unwrap_or_default()
            && cause.input_digest == input_digest
    }

    /// Validate a typed cause and exact canonical input against this durable result.
    #[must_use]
    pub fn validates_continuation_cause(
        &self,
        cause: &BackgroundSubagentContinuationCause,
        run: &RunRecord,
        artifact_content: Option<&str>,
    ) -> bool {
        let artifact_shape_matches = match self.retention_status {
            DurableBackgroundSubagentRetentionStatus::Artifact => artifact_content.is_some(),
            DurableBackgroundSubagentRetentionStatus::Inline
            | DurableBackgroundSubagentRetentionStatus::Expired => artifact_content.is_none(),
        };
        if !artifact_shape_matches || !self.validates_continuation_cause_envelope(cause, run) {
            return false;
        }
        run.input == self.continuation_input(artifact_content)
    }
}

/// Typed immutable cause for one result-triggered continuation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BackgroundSubagentContinuationCause {
    /// Terminal attempt whose result caused the continuation.
    pub attempt_id: SubagentAttemptId,
    /// Stable child conversation identity.
    pub agent_id: String,
    /// Owning parent session.
    pub parent_session_id: SessionId,
    /// Parent run that accepted the delegation.
    pub parent_run_id: RunId,
    /// Child runtime run, when one was created.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_run_id: Option<RunId>,
    /// Digest of the complete logical terminal result, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_digest: Option<String>,
    /// Complete logical terminal-result size.
    pub result_size_bytes: u64,
    /// Parent trace context inherited by the continuation.
    #[serde(default, skip_serializing_if = "TraceContext::is_empty")]
    pub trace_context: TraceContext,
    /// Digest of the exact canonical durable continuation input.
    pub input_digest: String,
}

impl BackgroundSubagentContinuationCause {
    /// Build the canonical cause for a record and already-resolved continuation input.
    ///
    /// # Errors
    ///
    /// Returns a serialization error when the canonical durable input cannot be encoded.
    pub fn new(
        record: &BackgroundSubagentRecord,
        input: &[InputPart],
    ) -> Result<Self, serde_json::Error> {
        Ok(Self {
            attempt_id: record.attempt_id.clone(),
            agent_id: record.agent_id.clone(),
            parent_session_id: record.parent_session_id.clone(),
            parent_run_id: record.parent_run_id.clone(),
            child_run_id: record.child_run_id.clone(),
            result_digest: record
                .result_ref
                .as_ref()
                .and_then(|result| result.digest.clone()),
            result_size_bytes: record
                .result_ref
                .as_ref()
                .map_or(0, |result| result.size_bytes),
            trace_context: record.trace_context.clone().unwrap_or_default(),
            input_digest: background_subagent_input_digest(input)?,
        })
    }
}

/// Compute the versioned domain-separated digest for one logical terminal result.
#[must_use]
pub fn background_subagent_result_digest(content: Option<&str>, error: Option<&str>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"starweaver.session.background_subagent.result.v1\0");
    match (content, error) {
        (Some(content), _) => {
            hasher.update(b"content\0");
            hasher.update(content.as_bytes());
        }
        (None, Some(error)) => {
            hasher.update(b"error\0");
            hasher.update(error.as_bytes());
        }
        (None, None) => hasher.update(b"empty\0"),
    }
    format!("sha256:{:x}", hasher.finalize())
}

/// Compute a domain-separated SHA-256 digest of canonical durable input parts.
///
/// # Errors
///
/// Returns a serialization error when the canonical durable input cannot be encoded.
pub fn background_subagent_input_digest(input: &[InputPart]) -> Result<String, serde_json::Error> {
    let payload = serde_json::to_vec(input)?;
    let mut hasher = Sha256::new();
    hasher.update(b"starweaver.session.background_subagent_continuation.input.v1\0");
    hasher.update(payload);
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

/// Atomic durable continuation admission request.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AcquireBackgroundSubagentContinuation {
    /// Terminal attempt whose logical result becomes the continuation input.
    pub attempt_id: SubagentAttemptId,
    /// Stable delivery claim identity.
    pub claim_id: String,
    /// Claim deadline retained for audit and conflict handling.
    pub claim_deadline: DateTime<Utc>,
    /// Typed immutable cause bound to the exact canonical continuation input.
    pub cause: BackgroundSubagentContinuationCause,
    /// Ordinary single-active-run admission request containing the continuation run.
    pub admission: AcquireRunAdmission,
}

/// Atomic durable continuation admission receipt.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BackgroundSubagentContinuationReceipt {
    /// Store-derived typed cause attesting the admitted canonical input.
    pub cause: BackgroundSubagentContinuationCause,
    /// Background record after delivery ownership is linked to the continuation.
    pub background: BackgroundSubagentRecord,
    /// Ordinary durable run admission receipt.
    pub admission: RunAdmissionReceipt,
}
