//! In-memory session store implementation.

mod approvals;
mod checkpoints;
mod runs;
mod sessions;
mod streams;
mod traces;

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use starweaver_context::ResumableState;
use starweaver_core::{RunId, SessionId};
use starweaver_runtime::{AgentCheckpoint, AgentStreamRecord};

use crate::{
    approval::{ApprovalRecord, DeferredToolRecord},
    error::{SessionStoreError, SessionStoreResult},
    records::{
        EnvironmentStateRef, RunRecord, RunStatus, SessionRecord, SessionStatus, StreamCursorRef,
    },
    trace::{CompactRunTrace, CompactSessionTrace},
};

use super::{SessionFilter, SessionStore};

/// In-memory session store for deterministic tests and single-process hosts.
#[derive(Clone, Debug, Default)]
pub struct InMemorySessionStore {
    inner: Arc<Mutex<StoreInner>>,
}

#[derive(Clone, Debug, Default)]
struct StoreInner {
    sessions: BTreeMap<SessionId, SessionRecord>,
    runs: BTreeMap<(SessionId, RunId), RunRecord>,
    checkpoints: BTreeMap<(SessionId, RunId), Vec<AgentCheckpoint>>,
    streams: BTreeMap<(SessionId, RunId), Vec<AgentStreamRecord>>,
    approvals: BTreeMap<(SessionId, RunId), Vec<ApprovalRecord>>,
    deferred_tools: BTreeMap<(SessionId, RunId), Vec<DeferredToolRecord>>,
}

impl InMemorySessionStore {
    /// Create an empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

fn run_key(session_id: &SessionId, run_id: &RunId) -> (SessionId, RunId) {
    (session_id.clone(), run_id.clone())
}

fn run_key_label(session_id: &SessionId, run_id: &RunId) -> String {
    format!("{}:{}", session_id.as_str(), run_id.as_str())
}

#[allow(clippy::needless_pass_by_value)]
fn store_failed(
    error: std::sync::PoisonError<std::sync::MutexGuard<'_, StoreInner>>,
) -> SessionStoreError {
    SessionStoreError::Failed(error.to_string())
}

#[async_trait]
impl SessionStore for InMemorySessionStore {
    async fn save_session(&self, session: SessionRecord) -> SessionStoreResult<()> {
        self.save_session_record(session)
    }

    async fn load_session(&self, session_id: &SessionId) -> SessionStoreResult<SessionRecord> {
        self.load_session_record(session_id)
    }

    async fn list_sessions(&self, filter: SessionFilter) -> SessionStoreResult<Vec<SessionRecord>> {
        self.list_session_records(filter)
    }

    async fn update_session_status(
        &self,
        session_id: &SessionId,
        status: SessionStatus,
    ) -> SessionStoreResult<()> {
        self.set_session_status(session_id, status)
    }

    async fn save_context_state(
        &self,
        session_id: &SessionId,
        state: ResumableState,
    ) -> SessionStoreResult<()> {
        self.save_context_state_snapshot(session_id, state)
    }

    async fn save_environment_state(
        &self,
        session_id: &SessionId,
        environment_state: EnvironmentStateRef,
    ) -> SessionStoreResult<()> {
        self.save_environment_state_ref(session_id, environment_state)
    }

    async fn append_run(&self, run: RunRecord) -> SessionStoreResult<()> {
        self.append_run_record(run)
    }

    async fn load_run(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<RunRecord> {
        self.load_run_record(session_id, run_id)
    }

    async fn list_runs(&self, session_id: &SessionId) -> SessionStoreResult<Vec<RunRecord>> {
        self.list_run_records(session_id)
    }

    async fn update_run_status(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        status: RunStatus,
        output_preview: Option<String>,
    ) -> SessionStoreResult<()> {
        self.set_run_status(session_id, run_id, status, output_preview)
    }

    async fn append_checkpoint(
        &self,
        session_id: &SessionId,
        checkpoint: AgentCheckpoint,
    ) -> SessionStoreResult<()> {
        self.append_checkpoint_record(session_id, checkpoint)
    }

    async fn load_checkpoints(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<Vec<AgentCheckpoint>> {
        self.load_checkpoint_records(session_id, run_id)
    }

    async fn append_stream_records(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        records: Vec<AgentStreamRecord>,
    ) -> SessionStoreResult<()> {
        self.append_stream_record_batch(session_id, run_id, records)
    }

    async fn replay_stream_records(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<Vec<AgentStreamRecord>> {
        self.replay_stream_record_batch(session_id, run_id)
    }

    async fn save_stream_cursor(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        cursor: StreamCursorRef,
    ) -> SessionStoreResult<()> {
        self.save_stream_cursor_ref(session_id, run_id, cursor)
    }

    async fn append_approval(&self, approval: ApprovalRecord) -> SessionStoreResult<()> {
        self.append_approval_record(approval)
    }

    async fn load_approvals(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<Vec<ApprovalRecord>> {
        self.load_approval_records(session_id, run_id)
    }

    async fn append_deferred_tool(&self, record: DeferredToolRecord) -> SessionStoreResult<()> {
        self.append_deferred_tool_record(record)
    }

    async fn load_deferred_tools(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<Vec<DeferredToolRecord>> {
        self.load_deferred_tool_records(session_id, run_id)
    }

    async fn compact_run_trace(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<CompactRunTrace> {
        self.compact_run_trace_projection(session_id, run_id)
    }

    async fn compact_session_trace(
        &self,
        session_id: &SessionId,
    ) -> SessionStoreResult<CompactSessionTrace> {
        self.compact_session_trace_projection(session_id)
    }
}
