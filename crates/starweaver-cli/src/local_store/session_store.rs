//! CLI adapter over the shared `SQLite` session store.

use async_trait::async_trait;
use starweaver_context::ResumableState;
use starweaver_core::{RunId, SessionId};
use starweaver_runtime::{AgentCheckpoint, AgentStreamRecord};
use starweaver_session::{
    AcquireRunAdmission, ApprovalRecord, CompactRunTrace, CompactSessionTrace, DeferredToolRecord,
    DurableControlReceipt, EnvironmentStateRef, HitlResumeClaim, ManagedRunTarget,
    PendingStreamPublication, RunAdmissionLease, RunAdmissionReceipt, RunEvidenceCommit, RunRecord,
    RunStatus, SessionContinuationFence, SessionFilter, SessionRecord, SessionStatus, SessionStore,
    SessionStoreResult, StreamCursorRef, StreamPublicationTarget, UpdateManagedSession,
};
use starweaver_storage::SqliteSessionStore;

use crate::config::CliConfig;

/// Session store bound to a resolved CLI database path.
#[derive(Clone, Debug)]
pub struct LocalSessionStore {
    store: SqliteSessionStore,
}

impl LocalSessionStore {
    /// Open a CLI session-store adapter before it is used by async runtime code.
    pub fn new(config: CliConfig) -> SessionStoreResult<Self> {
        crate::config::ensure_config_dirs(&config)
            .map_err(|error| starweaver_session::SessionStoreError::Failed(error.to_string()))?;
        let database_path = config.database_path;
        Ok(Self {
            store: SqliteSessionStore::open(&database_path)?,
        })
    }
}

#[async_trait]
impl SessionStore for LocalSessionStore {
    async fn commit_run_evidence(
        &self,
        commit: RunEvidenceCommit,
    ) -> SessionStoreResult<RunRecord> {
        self.store.commit_run_evidence(commit).await
    }

    async fn commit_checkpoint(
        &self,
        session_id: &SessionId,
        checkpoint: AgentCheckpoint,
    ) -> SessionStoreResult<()> {
        self.store.commit_checkpoint(session_id, checkpoint).await
    }

    async fn claim_hitl_resume(&self, claim: HitlResumeClaim) -> SessionStoreResult<()> {
        self.store.claim_hitl_resume(claim).await
    }

    async fn mark_hitl_resume_started(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        claim_id: &str,
    ) -> SessionStoreResult<()> {
        self.store
            .mark_hitl_resume_started(session_id, run_id, claim_id)
            .await
    }

    async fn release_hitl_resume_claim(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        claim_id: &str,
    ) -> SessionStoreResult<()> {
        self.store
            .release_hitl_resume_claim(session_id, run_id, claim_id)
            .await
    }

    async fn pending_stream_publications(
        &self,
        session_id: &SessionId,
    ) -> SessionStoreResult<Vec<PendingStreamPublication>> {
        self.store.pending_stream_publications(session_id).await
    }

    async fn acknowledge_stream_publication(
        &self,
        publication_id: &str,
        target: StreamPublicationTarget,
    ) -> SessionStoreResult<()> {
        self.store
            .acknowledge_stream_publication(publication_id, target)
            .await
    }

    async fn create_session_idempotent(
        &self,
        session: SessionRecord,
        idempotency_key: &str,
        command_fingerprint: &str,
    ) -> SessionStoreResult<SessionRecord> {
        self.store
            .create_session_idempotent(session, idempotency_key, command_fingerprint)
            .await
    }

    async fn update_managed_session(
        &self,
        command: UpdateManagedSession,
        command_fingerprint: &str,
    ) -> SessionStoreResult<SessionRecord> {
        self.store
            .update_managed_session(command, command_fingerprint)
            .await
    }

    async fn acquire_session_deletion_fence(
        &self,
        session_id: &SessionId,
        expected_revision: u64,
        fence_id: &str,
        requested_by: &str,
        idempotency_key: &str,
        command_fingerprint: &str,
    ) -> SessionStoreResult<SessionRecord> {
        self.store
            .acquire_session_deletion_fence(
                session_id,
                expected_revision,
                fence_id,
                requested_by,
                idempotency_key,
                command_fingerprint,
            )
            .await
    }

    async fn tombstone_session(
        &self,
        session_id: &SessionId,
        fence_id: &str,
    ) -> SessionStoreResult<SessionRecord> {
        self.store.tombstone_session(session_id, fence_id).await
    }

    async fn session_continuation_fence(
        &self,
        namespace_id: &str,
        session_id: &SessionId,
    ) -> SessionStoreResult<SessionContinuationFence> {
        self.store
            .session_continuation_fence(namespace_id, session_id)
            .await
    }

    async fn acquire_run_admission(
        &self,
        request: AcquireRunAdmission,
    ) -> SessionStoreResult<RunAdmissionReceipt> {
        self.store.acquire_run_admission(request).await
    }

    async fn heartbeat_run_admission(
        &self,
        lease: &RunAdmissionLease,
        lease_expires_at: chrono::DateTime<chrono::Utc>,
    ) -> SessionStoreResult<RunAdmissionLease> {
        self.store
            .heartbeat_run_admission(lease, lease_expires_at)
            .await
    }

    async fn release_run_admission(&self, lease: &RunAdmissionLease) -> SessionStoreResult<()> {
        self.store.release_run_admission(lease).await
    }

    async fn load_run_admission(
        &self,
        target: &ManagedRunTarget,
    ) -> SessionStoreResult<Option<RunAdmissionLease>> {
        self.store.load_run_admission(target).await
    }

    async fn reconcile_expired_run_admissions(
        &self,
        namespace_id: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> SessionStoreResult<Vec<ManagedRunTarget>> {
        self.store
            .reconcile_expired_run_admissions(namespace_id, now)
            .await
    }

    async fn load_control_receipt(
        &self,
        target: &ManagedRunTarget,
        idempotency_key: &str,
    ) -> SessionStoreResult<Option<DurableControlReceipt>> {
        self.store
            .load_control_receipt(target, idempotency_key)
            .await
    }

    async fn reserve_control_receipt(
        &self,
        receipt: DurableControlReceipt,
    ) -> SessionStoreResult<DurableControlReceipt> {
        self.store.reserve_control_receipt(receipt).await
    }

    async fn update_control_receipt_state(
        &self,
        receipt_id: &str,
        state: &str,
    ) -> SessionStoreResult<DurableControlReceipt> {
        self.store
            .update_control_receipt_state(receipt_id, state)
            .await
    }

    async fn save_session(&self, session: SessionRecord) -> SessionStoreResult<()> {
        self.store.save_session(session).await
    }

    async fn load_session(&self, session_id: &SessionId) -> SessionStoreResult<SessionRecord> {
        self.store.load_session(session_id).await
    }

    async fn list_sessions(&self, filter: SessionFilter) -> SessionStoreResult<Vec<SessionRecord>> {
        self.store.list_sessions(filter).await
    }

    async fn update_session_status(
        &self,
        session_id: &SessionId,
        status: SessionStatus,
    ) -> SessionStoreResult<()> {
        self.store.update_session_status(session_id, status).await
    }

    async fn save_context_state(
        &self,
        session_id: &SessionId,
        state: ResumableState,
    ) -> SessionStoreResult<()> {
        self.store.save_context_state(session_id, state).await
    }

    async fn save_environment_state(
        &self,
        session_id: &SessionId,
        environment_state: EnvironmentStateRef,
    ) -> SessionStoreResult<()> {
        self.store
            .save_environment_state(session_id, environment_state)
            .await
    }

    async fn append_run(&self, run: RunRecord) -> SessionStoreResult<()> {
        self.store.append_run(run).await
    }

    async fn load_run(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<RunRecord> {
        self.store.load_run(session_id, run_id).await
    }

    async fn list_runs(&self, session_id: &SessionId) -> SessionStoreResult<Vec<RunRecord>> {
        self.store.list_runs(session_id).await
    }

    async fn update_run_status(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        status: RunStatus,
        output_preview: Option<String>,
    ) -> SessionStoreResult<()> {
        self.store
            .update_run_status(session_id, run_id, status, output_preview)
            .await
    }

    async fn append_checkpoint(
        &self,
        session_id: &SessionId,
        checkpoint: AgentCheckpoint,
    ) -> SessionStoreResult<()> {
        self.store.append_checkpoint(session_id, checkpoint).await
    }

    async fn load_checkpoints(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<Vec<AgentCheckpoint>> {
        self.store.load_checkpoints(session_id, run_id).await
    }

    async fn append_stream_records(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        records: Vec<AgentStreamRecord>,
    ) -> SessionStoreResult<()> {
        self.store
            .append_stream_records(session_id, run_id, records)
            .await
    }

    async fn replay_stream_records(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<Vec<AgentStreamRecord>> {
        self.store.replay_stream_records(session_id, run_id).await
    }

    async fn replay_stream_records_after(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        after_sequence: Option<usize>,
    ) -> SessionStoreResult<Vec<AgentStreamRecord>> {
        self.store
            .replay_stream_records_after(session_id, run_id, after_sequence)
            .await
    }

    async fn save_stream_cursor(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        cursor: StreamCursorRef,
    ) -> SessionStoreResult<()> {
        self.store
            .save_stream_cursor(session_id, run_id, cursor)
            .await
    }

    async fn append_approval(&self, approval: ApprovalRecord) -> SessionStoreResult<()> {
        self.store.append_approval(approval).await
    }

    async fn load_approvals(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<Vec<ApprovalRecord>> {
        self.store.load_approvals(session_id, run_id).await
    }

    async fn append_deferred_tool(&self, record: DeferredToolRecord) -> SessionStoreResult<()> {
        self.store.append_deferred_tool(record).await
    }

    async fn load_deferred_tools(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<Vec<DeferredToolRecord>> {
        self.store.load_deferred_tools(session_id, run_id).await
    }

    async fn compact_run_trace(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<CompactRunTrace> {
        self.store.compact_run_trace(session_id, run_id).await
    }

    async fn compact_session_trace(
        &self,
        session_id: &SessionId,
    ) -> SessionStoreResult<CompactSessionTrace> {
        self.store.compact_session_trace(session_id).await
    }
}
