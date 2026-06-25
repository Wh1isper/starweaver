//! Durable session store contract.

use async_trait::async_trait;
use starweaver_context::ResumableState;
use starweaver_core::{RunId, SessionId};
use starweaver_runtime::{AgentCheckpoint, AgentStreamRecord};

use crate::{
    approval::{ApprovalRecord, DeferredToolRecord},
    error::SessionStoreResult,
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
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<SessionResumeSnapshot> {
        let session = self.load_session(session_id).await?;
        let run = self.load_run(session_id, run_id).await?;
        let latest_checkpoint = self.latest_checkpoint(session_id, run_id).await?;
        let after_sequence = latest_checkpoint
            .as_ref()
            .and_then(|checkpoint| checkpoint.resume.cursor.stream_cursor);
        let stream_records = self
            .replay_stream_records_after(session_id, run_id, after_sequence)
            .await?;
        let approvals = self.load_approvals(session_id, run_id).await?;
        let deferred_tools = self.load_deferred_tools(session_id, run_id).await?;
        let environment_state = run
            .environment_state
            .clone()
            .or_else(|| session.environment_state.clone());
        let mut stream_cursors = session.stream_cursors.clone();
        stream_cursors.extend(run.stream_cursors.clone());
        Ok(SessionResumeSnapshot {
            state: session.state.clone(),
            session,
            run,
            environment_state,
            latest_checkpoint,
            stream_records,
            approvals,
            deferred_tools,
            stream_cursors,
        })
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
