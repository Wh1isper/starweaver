//! Durable session store contract.

use async_trait::async_trait;
use starweaver_context::{AgentCheckpoint, ResumableState};
use starweaver_core::{RunId, SessionId};
use starweaver_stream::AgentStreamRecord;

use crate::{
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

    /// Acquire exclusive ownership of a waiting run before any continuation side effect.
    ///
    /// A run may have at most one active claim. Claims are consumed by the related-run update in
    /// [`Self::commit_run_evidence`].
    async fn claim_hitl_resume(&self, _claim: HitlResumeClaim) -> SessionStoreResult<()> {
        Err(SessionStoreError::Failed(
            "session store does not support exclusive HITL resume claims".to_string(),
        ))
    }

    /// Mark a claim started immediately before the first continuation hook or tool executes.
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
