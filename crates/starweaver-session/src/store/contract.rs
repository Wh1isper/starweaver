//! Durable session store contract.

use async_trait::async_trait;
use starweaver_context::{AgentCheckpoint, ResumableState};
use starweaver_core::{RunId, SessionId};
use starweaver_stream::{AgentStreamRecord, ReplayEvent};

use crate::{
    AcquireBackgroundSubagentContinuation, AcquireRunAdmission,
    BackgroundSubagentContinuationReceipt, BackgroundSubagentRecord,
    DurableBackgroundSubagentDeliveryClaim, DurableBackgroundSubagentDeliveryRelease,
    DurableControlReceipt, RunAdmissionLease, RunAdmissionReceipt, SessionContinuationFence,
    UpdateManagedSession,
    approval::{ApprovalRecord, DeferredToolRecord},
    claim::HitlResumeClaim,
    error::{SessionStoreError, SessionStoreResult},
    evidence::RunEvidenceCommit,
    publication::{PendingStreamPublication, StreamPublicationTarget},
    records::{
        EnvironmentStateRef, RunRecord, RunStatus, SessionRecord, SessionStatus, StreamCursorRef,
    },
    resume::SessionResumeSnapshot,
    trace::{CompactRunTrace, CompactSessionTrace},
};

fn management_unsupported<T>() -> SessionStoreResult<T> {
    Err(SessionStoreError::Failed(
        "session store does not support agent session management".to_string(),
    ))
}

/// Query filters for listing sessions.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SessionFilter {
    /// Required session status.
    pub status: Option<SessionStatus>,
    /// Required profile name.
    pub profile: Option<String>,
    /// Required workspace identifier or path.
    pub workspace: Option<String>,
    /// Maximum number of sessions returned.
    pub limit: Option<usize>,
}

/// Durable session store contract.
#[async_trait]
pub trait SessionStore: Send + Sync {
    /// Atomically persist a complete run evidence bundle.
    ///
    /// Implementations must leave either the complete previous state or the complete committed
    /// state visible. Identical retries are idempotent and conflicting retries must fail.
    async fn commit_run_evidence(&self, commit: RunEvidenceCommit)
    -> SessionStoreResult<RunRecord>;

    /// Atomically persist a complete run evidence bundle under an active admission lease.
    ///
    /// Implementations must validate the admission identity and fencing generation in the same
    /// transaction as the evidence write. Exact evidence retries may succeed after release, but
    /// a new or conflicting write from an expired or stale owner must fail.
    async fn commit_run_evidence_fenced(
        &self,
        _lease: &RunAdmissionLease,
        _commit: RunEvidenceCommit,
    ) -> SessionStoreResult<RunRecord> {
        management_unsupported()
    }

    /// Atomically append a replay-event batch under an active admission lease.
    ///
    /// Every event must use the run-local scope for the admitted run. Implementations must
    /// validate the active admission identity, host, target, generation, and expiry in the same
    /// transaction as the inserts. Exact retries are idempotent; a different event at an occupied
    /// sequence conflicts and leaves the entire batch unchanged.
    async fn append_replay_events_fenced(
        &self,
        _lease: &RunAdmissionLease,
        _events: Vec<ReplayEvent>,
    ) -> SessionStoreResult<()> {
        management_unsupported()
    }

    /// Atomically bootstrap missing session/run records and persist one runtime checkpoint.
    ///
    /// This is the executor write path. Implementations must not expose a session or run without
    /// the checkpoint when the operation fails. Exact checkpoint retries are idempotent and
    /// conflicting retries fail.
    async fn commit_checkpoint(
        &self,
        session_id: &SessionId,
        checkpoint: AgentCheckpoint,
    ) -> SessionStoreResult<()>;

    /// Persist one runtime checkpoint under an active admission lease.
    ///
    /// The store must validate the lease in the same transaction as a new checkpoint write.
    async fn commit_checkpoint_fenced(
        &self,
        _lease: &RunAdmissionLease,
        _checkpoint: AgentCheckpoint,
    ) -> SessionStoreResult<()> {
        management_unsupported()
    }

    /// Acquire exclusive ownership of a waiting run before any continuation side effect.
    ///
    /// A run may have at most one active claim. Claims are consumed by the related-run update in
    /// [`Self::commit_run_evidence`].
    async fn claim_hitl_resume(&self, _claim: HitlResumeClaim) -> SessionStoreResult<()> {
        Err(SessionStoreError::Failed(
            "session store does not support exclusive HITL resume claims".to_string(),
        ))
    }

    /// Atomically authorize the first HITL continuation effect under a live admission lease.
    ///
    /// Stores must verify the exact current lease, its expiry, the target run's source binding,
    /// and the matching `Admitted` claim in one operation before advancing the claim to `Started`.
    async fn start_hitl_resume_effect(
        &self,
        _lease: &RunAdmissionLease,
        _source_run_id: &RunId,
        _claim_id: &str,
    ) -> SessionStoreResult<()> {
        management_unsupported()
    }

    /// Atomically abort a pre-worker waiting-run replacement under its live admission lease.
    ///
    /// An `Admitted` claim proves no approved effect can have run, so this terminalizes only the
    /// replacement and consumes the claim while leaving the source waiting. `Started` is reported
    /// without mutation: callers must instead persist fail-closed related-run evidence.
    async fn abort_admitted_hitl_resume(
        &self,
        _lease: &RunAdmissionLease,
        _source_run_id: &RunId,
        _claim_id: &str,
        _output_preview: &str,
    ) -> SessionStoreResult<crate::HitlResumeAbortOutcome> {
        management_unsupported()
    }

    /// Mark a non-admitted claim started immediately before the first continuation hook or tool executes.
    async fn mark_hitl_resume_started(
        &self,
        _session_id: &SessionId,
        _run_id: &RunId,
        _claim_id: &str,
    ) -> SessionStoreResult<()> {
        Err(SessionStoreError::Failed(
            "session store does not support exclusive HITL resume claims".to_string(),
        ))
    }

    /// Release a preflight claim. Stores must reject release after execution has started.
    async fn release_hitl_resume_claim(
        &self,
        _session_id: &SessionId,
        _run_id: &RunId,
        _claim_id: &str,
    ) -> SessionStoreResult<()> {
        Err(SessionStoreError::Failed(
            "session store does not support exclusive HITL resume claims".to_string(),
        ))
    }

    /// List transactionally enqueued stream publications still awaiting at least one sink.
    async fn pending_stream_publications(
        &self,
        _session_id: &SessionId,
    ) -> SessionStoreResult<Vec<PendingStreamPublication>> {
        Err(SessionStoreError::Failed(
            "session store does not support transactional stream publication".to_string(),
        ))
    }

    /// Acknowledge one sink only after its complete idempotent delivery succeeds.
    async fn acknowledge_stream_publication(
        &self,
        _publication_id: &str,
        _target: StreamPublicationTarget,
    ) -> SessionStoreResult<()> {
        Err(SessionStoreError::Failed(
            "session store does not support transactional stream publication".to_string(),
        ))
    }

    /// Atomically create a session and bind an idempotency key to a normalized fingerprint.
    async fn create_session_idempotent(
        &self,
        _session: SessionRecord,
        _idempotency_key: &str,
        _command_fingerprint: &str,
    ) -> SessionStoreResult<SessionRecord> {
        management_unsupported()
    }

    /// Apply an allowlisted session patch with expected-revision and idempotency checks.
    async fn update_managed_session(
        &self,
        _command: UpdateManagedSession,
        _command_fingerprint: &str,
    ) -> SessionStoreResult<SessionRecord> {
        management_unsupported()
    }

    /// Acquire a deletion fence that blocks run, continuation, and delegation admission.
    async fn acquire_session_deletion_fence(
        &self,
        _session_id: &SessionId,
        _expected_revision: u64,
        _fence_id: &str,
        _requested_by: &str,
        _idempotency_key: &str,
        _command_fingerprint: &str,
    ) -> SessionStoreResult<SessionRecord> {
        management_unsupported()
    }

    /// Complete a fenced session tombstone. This never purges retained evidence.
    async fn tombstone_session(
        &self,
        _session_id: &SessionId,
        _fence_id: &str,
    ) -> SessionStoreResult<SessionRecord> {
        management_unsupported()
    }

    /// Load the deletion/continuation fence used by async supervisors before side effects.
    async fn session_continuation_fence(
        &self,
        _namespace_id: &str,
        _session_id: &SessionId,
    ) -> SessionStoreResult<SessionContinuationFence> {
        management_unsupported()
    }

    /// Atomically persist a queued run and acquire the session's single active lease.
    async fn acquire_run_admission(
        &self,
        _request: AcquireRunAdmission,
    ) -> SessionStoreResult<RunAdmissionReceipt> {
        management_unsupported()
    }

    /// Read an admission idempotency receipt without creating or changing durable state.
    async fn load_run_admission_receipt(
        &self,
        _namespace_id: &str,
        _idempotency_key: &str,
        _command_fingerprint: &str,
    ) -> SessionStoreResult<Option<RunAdmissionReceipt>> {
        management_unsupported()
    }

    /// Extend a lease only for its current host and fencing generation.
    async fn heartbeat_run_admission(
        &self,
        _lease: &RunAdmissionLease,
        _lease_expires_at: chrono::DateTime<chrono::Utc>,
    ) -> SessionStoreResult<RunAdmissionLease> {
        management_unsupported()
    }

    /// Release a lease after terminal run durability; stale generations cannot release a new owner.
    async fn release_run_admission(&self, _lease: &RunAdmissionLease) -> SessionStoreResult<()> {
        management_unsupported()
    }

    /// Update one admitted run while validating its active lease in the same transaction.
    async fn update_run_status_fenced(
        &self,
        _lease: &RunAdmissionLease,
        _status: RunStatus,
        _output_preview: Option<String>,
    ) -> SessionStoreResult<RunRecord> {
        management_unsupported()
    }

    /// Atomically persist a non-active status and release its matching admission lease.
    ///
    /// If complete terminal evidence was already committed under the active lease, that evidence
    /// is authoritative: finalization releases only the matching lease and ignores a differing
    /// process-local fallback outcome. An exact retry after a successful commit is idempotent. A
    /// stale owner cannot overwrite a different terminal result or release a newer generation.
    async fn finalize_run_admission(
        &self,
        _lease: &RunAdmissionLease,
        _status: RunStatus,
        _output_preview: Option<String>,
    ) -> SessionStoreResult<RunRecord> {
        management_unsupported()
    }

    /// Load durable admission truth for a composite target.
    async fn load_run_admission(
        &self,
        _target: &crate::ManagedRunTarget,
    ) -> SessionStoreResult<Option<RunAdmissionLease>> {
        management_unsupported()
    }

    /// Deterministically terminalize expired active leases owned by prior host instances.
    ///
    /// When an expired lease belongs to a waiting-HITL replacement whose source still waits, the
    /// replacement and source must both become cancelled, the exact started source claim must be
    /// validated and consumed, the admission must be removed, and session active-run state must
    /// be cleared as one atomic operation. Any mismatch fails closed without exposing a partial
    /// transition. Ordinary expired admissions retain the same replacement-only terminalization.
    async fn reconcile_expired_run_admissions(
        &self,
        _namespace_id: &str,
        _now: chrono::DateTime<chrono::Utc>,
    ) -> SessionStoreResult<Vec<crate::ManagedRunTarget>> {
        management_unsupported()
    }

    /// Load a durable control receipt by composite target and idempotency key.
    async fn load_control_receipt(
        &self,
        _target: &crate::ManagedRunTarget,
        _idempotency_key: &str,
    ) -> SessionStoreResult<Option<DurableControlReceipt>> {
        management_unsupported()
    }

    /// Reserve or replay a durable fenced control receipt.
    async fn reserve_control_receipt(
        &self,
        _receipt: DurableControlReceipt,
    ) -> SessionStoreResult<DurableControlReceipt> {
        management_unsupported()
    }

    /// Record the final accepted/failed effect state for a reserved receipt.
    async fn update_control_receipt_state(
        &self,
        _receipt_id: &str,
        _state: &str,
    ) -> SessionStoreResult<DurableControlReceipt> {
        management_unsupported()
    }

    /// Wait for store-owned background-subagent operations whose caller futures may have ended.
    ///
    /// Implementations that detach non-cancellable database or network work must retain and drain
    /// that work here. Cancellation-safe implementations must explicitly return success; the
    /// default fails closed so a store cannot accidentally claim a complete shutdown guarantee.
    async fn drain_background_subagent_operations(&self) -> SessionStoreResult<()> {
        management_unsupported()
    }

    /// Idempotently persist one accepted durable background-subagent attempt.
    async fn record_background_subagent_acceptance(
        &self,
        _record: BackgroundSubagentRecord,
    ) -> SessionStoreResult<BackgroundSubagentRecord> {
        management_unsupported()
    }

    /// Persist a monotonic non-terminal lifecycle transition or child-run correlation.
    async fn update_background_subagent_execution(
        &self,
        _record: BackgroundSubagentRecord,
    ) -> SessionStoreResult<BackgroundSubagentRecord> {
        management_unsupported()
    }

    /// Extend an active background execution lease for its current fenced owner.
    async fn heartbeat_background_subagent(
        &self,
        _attempt_id: &starweaver_core::SubagentAttemptId,
        _host_instance_id: &str,
        _fencing_generation: u64,
        _lease_expires_at: chrono::DateTime<chrono::Utc>,
    ) -> SessionStoreResult<BackgroundSubagentRecord> {
        management_unsupported()
    }

    /// Atomically persist terminal evidence and its optional oversized-result artifact.
    async fn commit_background_subagent_terminal(
        &self,
        commit: crate::BackgroundSubagentTerminalCommit,
    ) -> SessionStoreResult<BackgroundSubagentRecord> {
        if commit.artifact.is_some() {
            return management_unsupported();
        }
        self.record_background_subagent_terminal(commit.record)
            .await
    }

    /// Load one retained background-result artifact by stable reference.
    async fn load_background_subagent_artifact(
        &self,
        _artifact_ref: &str,
    ) -> SessionStoreResult<crate::BackgroundSubagentArtifact> {
        management_unsupported()
    }

    /// Expire retained background-result content while preserving minimal audit evidence.
    async fn expire_background_subagent_retention(
        &self,
        _namespace_id: &str,
        _now: chrono::DateTime<chrono::Utc>,
        _limit: usize,
    ) -> SessionStoreResult<Vec<BackgroundSubagentRecord>> {
        management_unsupported()
    }

    /// Idempotently persist immutable terminal outcome before delivery becomes claimable.
    async fn record_background_subagent_terminal(
        &self,
        _record: BackgroundSubagentRecord,
    ) -> SessionStoreResult<BackgroundSubagentRecord> {
        management_unsupported()
    }

    /// Load one durable background attempt by globally unique attempt identity.
    async fn load_background_subagent(
        &self,
        _attempt_id: &starweaver_core::SubagentAttemptId,
    ) -> SessionStoreResult<BackgroundSubagentRecord> {
        management_unsupported()
    }

    /// List bounded durable background attempts in one host namespace and optional session.
    async fn list_background_subagents(
        &self,
        _namespace_id: &str,
        _session_id: Option<&SessionId>,
        _limit: usize,
    ) -> SessionStoreResult<Vec<BackgroundSubagentRecord>> {
        management_unsupported()
    }

    /// List terminal results still awaiting or holding logical delivery ownership.
    async fn list_pending_background_subagents(
        &self,
        _namespace_id: &str,
        _session_id: Option<&SessionId>,
        _limit: usize,
    ) -> SessionStoreResult<Vec<BackgroundSubagentRecord>> {
        management_unsupported()
    }

    /// Atomically claim one terminal result, allowing exact-claim idempotent replay.
    async fn claim_background_subagent_delivery(
        &self,
        _attempt_id: &starweaver_core::SubagentAttemptId,
        _claim: DurableBackgroundSubagentDeliveryClaim,
    ) -> SessionStoreResult<BackgroundSubagentRecord> {
        management_unsupported()
    }

    /// Acknowledge one matching claim as logically delivered.
    async fn acknowledge_background_subagent_delivery(
        &self,
        _attempt_id: &starweaver_core::SubagentAttemptId,
        _claim_id: &str,
    ) -> SessionStoreResult<BackgroundSubagentRecord> {
        management_unsupported()
    }

    /// Release one matching claim with a durable retry or consumer-termination disposition.
    async fn release_background_subagent_delivery(
        &self,
        _attempt_id: &starweaver_core::SubagentAttemptId,
        _claim_id: &str,
        _release: DurableBackgroundSubagentDeliveryRelease,
    ) -> SessionStoreResult<BackgroundSubagentRecord> {
        management_unsupported()
    }

    /// Atomically admit a continuation run and consume its background result exactly once.
    async fn acquire_background_subagent_continuation(
        &self,
        _request: AcquireBackgroundSubagentContinuation,
    ) -> SessionStoreResult<BackgroundSubagentContinuationReceipt> {
        management_unsupported()
    }

    /// Classify lost in-process executions and reclaim expired delivery claims after restart.
    async fn reconcile_background_subagents(
        &self,
        _namespace_id: &str,
        _now: chrono::DateTime<chrono::Utc>,
    ) -> SessionStoreResult<Vec<BackgroundSubagentRecord>> {
        management_unsupported()
    }

    /// Save a session record.
    async fn save_session(&self, session: SessionRecord) -> SessionStoreResult<()>;

    /// Load a session record.
    async fn load_session(&self, session_id: &SessionId) -> SessionStoreResult<SessionRecord>;

    /// List sessions by optional filter.
    async fn list_sessions(&self, filter: SessionFilter) -> SessionStoreResult<Vec<SessionRecord>>;

    /// Update session status.
    async fn update_session_status(
        &self,
        session_id: &SessionId,
        status: SessionStatus,
    ) -> SessionStoreResult<()>;

    /// Save a context state snapshot for a session.
    async fn save_context_state(
        &self,
        session_id: &SessionId,
        state: ResumableState,
    ) -> SessionStoreResult<()>;

    /// Save an environment state reference for a session.
    async fn save_environment_state(
        &self,
        session_id: &SessionId,
        environment_state: EnvironmentStateRef,
    ) -> SessionStoreResult<()>;

    /// Append or replace a run record.
    ///
    /// A zero `sequence_no` requests atomic session-local allocation. Replacing an existing run
    /// must preserve its assigned sequence; an explicit attempt to change that sequence fails.
    async fn append_run(&self, run: RunRecord) -> SessionStoreResult<()>;

    /// Load a run record.
    async fn load_run(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<RunRecord>;

    /// List runs for a session.
    async fn list_runs(&self, session_id: &SessionId) -> SessionStoreResult<Vec<RunRecord>>;

    /// Update run status and optional output preview.
    async fn update_run_status(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        status: RunStatus,
        output_preview: Option<String>,
    ) -> SessionStoreResult<()>;

    /// Append a full runtime checkpoint.
    async fn append_checkpoint(
        &self,
        session_id: &SessionId,
        checkpoint: AgentCheckpoint,
    ) -> SessionStoreResult<()>;

    /// Load checkpoints for a run in insertion order.
    async fn load_checkpoints(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<Vec<AgentCheckpoint>>;

    /// Load the latest checkpoint for a run.
    async fn latest_checkpoint(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<Option<AgentCheckpoint>> {
        let checkpoints = self.load_checkpoints(session_id, run_id).await?;
        Ok(checkpoints.into_iter().last())
    }

    /// Append runtime stream records used as resume evidence.
    async fn append_stream_records(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        records: Vec<AgentStreamRecord>,
    ) -> SessionStoreResult<()>;

    /// Replay runtime stream records for a run.
    async fn replay_stream_records(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<Vec<AgentStreamRecord>>;

    /// Replay runtime stream records after a sequence cursor.
    async fn replay_stream_records_after(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        after_sequence: Option<usize>,
    ) -> SessionStoreResult<Vec<AgentStreamRecord>> {
        let records = self.replay_stream_records(session_id, run_id).await?;
        Ok(records
            .into_iter()
            .filter(|record| after_sequence.is_none_or(|cursor| record.sequence > cursor))
            .collect())
    }

    /// Store a stream cursor reference for a run and session.
    async fn save_stream_cursor(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        cursor: StreamCursorRef,
    ) -> SessionStoreResult<()>;

    /// Append an approval record.
    async fn append_approval(&self, approval: ApprovalRecord) -> SessionStoreResult<()>;

    /// Load approval records for a run.
    async fn load_approvals(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<Vec<ApprovalRecord>>;

    /// Append a deferred tool record.
    async fn append_deferred_tool(&self, record: DeferredToolRecord) -> SessionStoreResult<()>;

    /// Load deferred tool records for a run.
    async fn load_deferred_tools(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<Vec<DeferredToolRecord>>;

    /// Load a resume snapshot from session, checkpoint, and stream evidence.
    async fn resume_snapshot(
        &self,
        _session_id: &SessionId,
        _run_id: &RunId,
    ) -> SessionStoreResult<SessionResumeSnapshot> {
        Err(SessionStoreError::Failed(
            "session store does not support per-run resume snapshots".to_string(),
        ))
    }

    /// Load and side-effect-free prepare one host-neutral continuation package.
    ///
    /// Implementations should normally rely on this default so every product applies the same
    /// snapshot identity and waiting-HITL evidence validation before admission or claim changes.
    async fn prepare_continuation(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        mode: crate::ContinuationPreparationMode,
    ) -> SessionStoreResult<crate::PreparedContinuation> {
        let snapshot = self.resume_snapshot(session_id, run_id).await?;
        match mode {
            crate::ContinuationPreparationMode::Ordinary => {
                crate::PreparedContinuation::ordinary(snapshot)
            }
            crate::ContinuationPreparationMode::WaitingHitl => {
                crate::PreparedContinuation::waiting_hitl(snapshot)
            }
        }
        .map_err(|error| SessionStoreError::Conflict(error.to_string()))
    }

    /// Return compact run trace projection.
    async fn compact_run_trace(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<CompactRunTrace>;

    /// Return compact session trace projection.
    async fn compact_session_trace(
        &self,
        session_id: &SessionId,
    ) -> SessionStoreResult<CompactSessionTrace>;
}
