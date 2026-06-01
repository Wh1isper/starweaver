//! Session store trait and in-memory implementation.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use chrono::Utc;
use starweaver_context::ResumableState;
use starweaver_core::{RunId, SessionId};
use starweaver_runtime::{AgentCheckpoint, AgentStreamRecord};

use crate::{
    approval::{ApprovalRecord, ApprovalStatus, DeferredToolRecord},
    error::{SessionStoreError, SessionStoreResult},
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
            .filter(|record| after_sequence.map_or(true, |cursor| record.sequence > cursor))
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
    async fn save_session(&self, mut session: SessionRecord) -> SessionStoreResult<()> {
        let mut inner = self.inner.lock().map_err(store_failed)?;
        session.updated_at = Utc::now();
        inner.sessions.insert(session.session_id.clone(), session);
        Ok(())
    }

    async fn load_session(&self, session_id: &SessionId) -> SessionStoreResult<SessionRecord> {
        let inner = self.inner.lock().map_err(store_failed)?;
        inner
            .sessions
            .get(session_id)
            .cloned()
            .ok_or_else(|| SessionStoreError::NotFound(session_id.as_str().to_string()))
    }

    async fn list_sessions(&self, filter: SessionFilter) -> SessionStoreResult<Vec<SessionRecord>> {
        let inner = self.inner.lock().map_err(store_failed)?;
        let mut sessions = inner
            .sessions
            .values()
            .filter(|session| {
                filter
                    .status
                    .map_or(true, |status| session.status == status)
            })
            .filter(|session| {
                filter
                    .profile
                    .as_ref()
                    .map_or(true, |profile| session.profile.as_ref() == Some(profile))
            })
            .filter(|session| {
                filter.workspace.as_ref().map_or(true, |workspace| {
                    session.workspace.as_ref() == Some(workspace)
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        sessions.sort_by_key(|session| std::cmp::Reverse(session.updated_at));
        if let Some(limit) = filter.limit {
            sessions.truncate(limit);
        }
        Ok(sessions)
    }

    async fn update_session_status(
        &self,
        session_id: &SessionId,
        status: SessionStatus,
    ) -> SessionStoreResult<()> {
        let mut inner = self.inner.lock().map_err(store_failed)?;
        let session = inner
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| SessionStoreError::NotFound(session_id.as_str().to_string()))?;
        session.status = status;
        session.updated_at = Utc::now();
        Ok(())
    }

    async fn save_context_state(
        &self,
        session_id: &SessionId,
        state: ResumableState,
    ) -> SessionStoreResult<()> {
        let mut inner = self.inner.lock().map_err(store_failed)?;
        let session = inner
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| SessionStoreError::NotFound(session_id.as_str().to_string()))?;
        session.state = state;
        session.updated_at = Utc::now();
        Ok(())
    }

    async fn save_environment_state(
        &self,
        session_id: &SessionId,
        environment_state: EnvironmentStateRef,
    ) -> SessionStoreResult<()> {
        let mut inner = self.inner.lock().map_err(store_failed)?;
        let session = inner
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| SessionStoreError::NotFound(session_id.as_str().to_string()))?;
        session.environment_state = Some(environment_state);
        session.updated_at = Utc::now();
        Ok(())
    }

    async fn append_run(&self, mut run: RunRecord) -> SessionStoreResult<()> {
        let mut inner = self.inner.lock().map_err(store_failed)?;
        run.updated_at = Utc::now();
        if !inner.sessions.contains_key(&run.session_id) {
            return Err(SessionStoreError::NotFound(
                run.session_id.as_str().to_string(),
            ));
        }
        inner
            .runs
            .insert(run_key(&run.session_id, &run.run_id), run.clone());
        if let Some(session) = inner.sessions.get_mut(&run.session_id) {
            session.head_run_id = Some(run.run_id.clone());
            if matches!(
                run.status,
                RunStatus::Queued | RunStatus::Running | RunStatus::Waiting
            ) {
                session.active_run_id = Some(run.run_id.clone());
            }
            if run.status == RunStatus::Completed {
                session.head_success_run_id = Some(run.run_id.clone());
                if session.active_run_id.as_ref() == Some(&run.run_id) {
                    session.active_run_id = None;
                }
            }
            session.updated_at = run.updated_at;
        }
        Ok(())
    }

    async fn load_run(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<RunRecord> {
        let inner = self.inner.lock().map_err(store_failed)?;
        let key = run_key(session_id, run_id);
        inner
            .runs
            .get(&key)
            .cloned()
            .ok_or_else(|| SessionStoreError::NotFound(run_key_label(session_id, run_id)))
    }

    async fn list_runs(&self, session_id: &SessionId) -> SessionStoreResult<Vec<RunRecord>> {
        let inner = self.inner.lock().map_err(store_failed)?;
        let mut runs = inner
            .runs
            .iter()
            .filter(|((stored_session_id, _run_id), _run)| stored_session_id == session_id)
            .map(|(_key, run)| run.clone())
            .collect::<Vec<_>>();
        runs.sort_by_key(|run| run.created_at);
        Ok(runs)
    }

    async fn update_run_status(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        status: RunStatus,
        output_preview: Option<String>,
    ) -> SessionStoreResult<()> {
        let mut inner = self.inner.lock().map_err(store_failed)?;
        let key = run_key(session_id, run_id);
        let updated_at = Utc::now();
        let run = inner
            .runs
            .get_mut(&key)
            .ok_or_else(|| SessionStoreError::NotFound(run_key_label(session_id, run_id)))?;
        run.status = status;
        run.output_preview = output_preview;
        run.updated_at = updated_at;
        if let Some(session) = inner.sessions.get_mut(session_id) {
            session.head_run_id = Some(run_id.clone());
            match status {
                RunStatus::Queued | RunStatus::Running | RunStatus::Waiting => {
                    session.active_run_id = Some(run_id.clone());
                }
                RunStatus::Completed => {
                    session.head_success_run_id = Some(run_id.clone());
                    if session.active_run_id.as_ref() == Some(run_id) {
                        session.active_run_id = None;
                    }
                }
                RunStatus::Failed | RunStatus::Cancelled => {
                    if session.active_run_id.as_ref() == Some(run_id) {
                        session.active_run_id = None;
                    }
                }
            }
            session.updated_at = updated_at;
        }
        Ok(())
    }

    async fn append_checkpoint(
        &self,
        session_id: &SessionId,
        checkpoint: AgentCheckpoint,
    ) -> SessionStoreResult<()> {
        let mut inner = self.inner.lock().map_err(store_failed)?;
        let key = run_key(session_id, &checkpoint.run_id);
        if !inner.runs.contains_key(&key) {
            return Err(SessionStoreError::NotFound(run_key_label(
                session_id,
                &checkpoint.run_id,
            )));
        }
        inner
            .checkpoints
            .entry(key.clone())
            .or_default()
            .push(checkpoint.clone());
        if let Some(run) = inner.runs.get_mut(&key) {
            run.latest_checkpoint = Some(crate::records::CheckpointRef {
                checkpoint_id: checkpoint.checkpoint_id,
                run_id: checkpoint.run_id,
                sequence: checkpoint.run_step,
                node: format!("{:?}", checkpoint.node),
                storage_ref: None,
                stream_cursor: checkpoint.resume.cursor.stream_cursor,
                created_at: Utc::now(),
                metadata: checkpoint.metadata,
            });
            run.updated_at = Utc::now();
        }
        Ok(())
    }

    async fn load_checkpoints(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<Vec<AgentCheckpoint>> {
        let inner = self.inner.lock().map_err(store_failed)?;
        Ok(inner
            .checkpoints
            .get(&run_key(session_id, run_id))
            .cloned()
            .unwrap_or_default())
    }

    async fn append_stream_records(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        records: Vec<AgentStreamRecord>,
    ) -> SessionStoreResult<()> {
        let mut inner = self.inner.lock().map_err(store_failed)?;
        let key = run_key(session_id, run_id);
        if !inner.runs.contains_key(&key) {
            return Err(SessionStoreError::NotFound(run_key_label(
                session_id, run_id,
            )));
        }
        let stream = inner.streams.entry(key.clone()).or_default();
        for record in records {
            if stream
                .iter()
                .all(|existing| existing.sequence != record.sequence)
            {
                stream.push(record);
            }
        }
        stream.sort_by_key(|record| record.sequence);
        let last_sequence = stream.last().map(|record| record.sequence);
        if let Some(run) = inner.runs.get_mut(&key) {
            if let Some(sequence) = last_sequence {
                let cursor = StreamCursorRef::new(
                    "raw_runtime",
                    format!("run:{}", run_id.as_str()),
                    sequence,
                );
                run.stream_cursors
                    .retain(|existing| existing.family != cursor.family);
                run.stream_cursors.push(cursor);
            }
            run.updated_at = Utc::now();
        }
        Ok(())
    }

    async fn replay_stream_records(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<Vec<AgentStreamRecord>> {
        let inner = self.inner.lock().map_err(store_failed)?;
        Ok(inner
            .streams
            .get(&run_key(session_id, run_id))
            .cloned()
            .unwrap_or_default())
    }

    async fn save_stream_cursor(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        cursor: StreamCursorRef,
    ) -> SessionStoreResult<()> {
        let mut inner = self.inner.lock().map_err(store_failed)?;
        let run_key = run_key(session_id, run_id);
        let updated_at = Utc::now();
        let run = inner
            .runs
            .get_mut(&run_key)
            .ok_or_else(|| SessionStoreError::NotFound(run_key_label(session_id, run_id)))?;
        run.stream_cursors
            .retain(|existing| existing.family != cursor.family || existing.scope != cursor.scope);
        run.stream_cursors.push(cursor.clone());
        run.updated_at = updated_at;
        if let Some(session) = inner.sessions.get_mut(session_id) {
            session.stream_cursors.retain(|existing| {
                existing.family != cursor.family || existing.scope != cursor.scope
            });
            session.stream_cursors.push(cursor);
            session.updated_at = updated_at;
        }
        Ok(())
    }

    async fn append_approval(&self, approval: ApprovalRecord) -> SessionStoreResult<()> {
        let mut inner = self.inner.lock().map_err(store_failed)?;
        let key = run_key(&approval.session_id, &approval.run_id);
        if !inner.runs.contains_key(&key) {
            return Err(SessionStoreError::NotFound(run_key_label(
                &approval.session_id,
                &approval.run_id,
            )));
        }
        inner.approvals.entry(key).or_default().push(approval);
        Ok(())
    }

    async fn load_approvals(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<Vec<ApprovalRecord>> {
        let inner = self.inner.lock().map_err(store_failed)?;
        Ok(inner
            .approvals
            .get(&run_key(session_id, run_id))
            .cloned()
            .unwrap_or_default())
    }

    async fn append_deferred_tool(&self, record: DeferredToolRecord) -> SessionStoreResult<()> {
        let mut inner = self.inner.lock().map_err(store_failed)?;
        let key = run_key(&record.session_id, &record.run_id);
        if !inner.runs.contains_key(&key) {
            return Err(SessionStoreError::NotFound(run_key_label(
                &record.session_id,
                &record.run_id,
            )));
        }
        inner.deferred_tools.entry(key).or_default().push(record);
        Ok(())
    }

    async fn load_deferred_tools(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<Vec<DeferredToolRecord>> {
        let inner = self.inner.lock().map_err(store_failed)?;
        Ok(inner
            .deferred_tools
            .get(&run_key(session_id, run_id))
            .cloned()
            .unwrap_or_default())
    }

    async fn compact_run_trace(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<CompactRunTrace> {
        let inner = self.inner.lock().map_err(store_failed)?;
        let key = run_key(session_id, run_id);
        let run = inner
            .runs
            .get(&key)
            .ok_or_else(|| SessionStoreError::NotFound(run_key_label(session_id, run_id)))?;
        let checkpoints = inner.checkpoints.get(&key).cloned().unwrap_or_default();
        let stream_cursor = inner
            .streams
            .get(&key)
            .and_then(|records| records.last())
            .map(|record| record.sequence);
        let approvals = inner.approvals.get(&key).map_or(0, |records| {
            records
                .iter()
                .filter(|record| record.status == ApprovalStatus::Pending)
                .count()
        });
        let deferred_tools = inner.deferred_tools.get(&key).map_or(0, Vec::len);
        Ok(CompactRunTrace {
            session_id: Some(session_id.clone()),
            run_id: Some(run_id.clone()),
            status: run.status,
            checkpoints: checkpoints
                .iter()
                .map(|checkpoint| checkpoint.checkpoint_id.clone())
                .collect(),
            approvals,
            deferred_tools,
            latest_checkpoint: checkpoints
                .last()
                .map(|checkpoint| checkpoint.checkpoint_id.clone()),
            stream_cursor,
            stream_cursors: run.stream_cursors.clone(),
            output_preview: run.output_preview.clone(),
            trace_context: run.trace_context.clone(),
            updated_at: Some(run.updated_at),
            metadata: run.metadata.clone(),
        })
    }

    async fn compact_session_trace(
        &self,
        session_id: &SessionId,
    ) -> SessionStoreResult<CompactSessionTrace> {
        let inner = self.inner.lock().map_err(store_failed)?;
        let session = inner
            .sessions
            .get(session_id)
            .ok_or_else(|| SessionStoreError::NotFound(session_id.as_str().to_string()))?;
        let mut runs = inner
            .runs
            .iter()
            .filter(|((stored_session_id, _run_id), _run)| stored_session_id == session_id)
            .map(|(_key, run)| run.clone())
            .collect::<Vec<_>>();
        runs.sort_by_key(|run| run.created_at);
        let latest_run = runs.last();
        Ok(CompactSessionTrace {
            session_id: session.session_id.clone(),
            title: session.title.clone(),
            workspace: session.workspace.clone(),
            profile: session.profile.clone(),
            status: session.status,
            runs: runs.len(),
            latest_run_id: latest_run.map(|run| run.run_id.clone()),
            last_output_preview: latest_run.and_then(|run| run.output_preview.clone()),
            stream_cursors: session.stream_cursors.clone(),
            trace_context: session.trace_context.clone(),
            created_at: session.created_at,
            updated_at: session.updated_at,
            metadata: session.metadata.clone(),
        })
    }
}

/// Executor adapter that persists runtime checkpoints into a session store.
#[derive(Clone)]
pub struct SessionStoreExecutor {
    store: Arc<dyn SessionStore>,
    session_id: SessionId,
}

impl SessionStoreExecutor {
    /// Create a checkpoint executor for one session.
    #[must_use]
    pub fn new(store: Arc<dyn SessionStore>, session_id: SessionId) -> Self {
        Self { store, session_id }
    }

    /// Return the session id associated with this executor.
    #[must_use]
    pub const fn session_id(&self) -> &SessionId {
        &self.session_id
    }
}

#[async_trait]
impl starweaver_runtime::AgentExecutor for SessionStoreExecutor {
    async fn checkpoint(
        &self,
        checkpoint: AgentCheckpoint,
    ) -> Result<starweaver_runtime::AgentExecutionDecision, starweaver_runtime::AgentExecutorError>
    {
        self.store
            .append_checkpoint(&self.session_id, checkpoint)
            .await?;
        Ok(starweaver_runtime::AgentExecutionDecision::Continue)
    }
}
