#![allow(clippy::significant_drop_tightening)]

//! Durable session runtime foundations for Starweaver.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use starweaver_context::ResumableState;
use starweaver_core::{CheckpointId, ConversationId, Metadata, RunId, TraceContext};
use starweaver_runtime::{
    AgentCheckpoint, AgentExecutionDecision, AgentExecutor, AgentExecutorError, AgentStreamRecord,
};
use thiserror::Error;

/// Session identifier.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct SessionId(String);

impl SessionId {
    /// Create an identifier from a caller-provided string.
    #[must_use]
    pub fn from_string(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Return string representation.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Durable session record.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SessionRecord {
    /// Session id.
    pub session_id: SessionId,
    /// Last exported context state.
    pub state: ResumableState,
    /// Session trace context.
    #[serde(default, skip_serializing_if = "TraceContext::is_empty")]
    pub trace_context: TraceContext,
    /// Metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

/// Durable run record.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunRecord {
    /// Session id.
    pub session_id: SessionId,
    /// Run id.
    pub run_id: RunId,
    /// Conversation id.
    pub conversation_id: ConversationId,
    /// Trace context.
    #[serde(default, skip_serializing_if = "TraceContext::is_empty")]
    pub trace_context: TraceContext,
    /// Metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

/// Compact run projection for tools, CLI, and UI.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct CompactRunTrace {
    /// Run id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    /// Checkpoint ids in insertion order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checkpoints: Vec<CheckpointId>,
    /// Stream event count.
    pub stream_events: usize,
    /// Latest resume checkpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_checkpoint: Option<CheckpointId>,
    /// Latest persisted stream cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_cursor: Option<usize>,
    /// Trace context.
    #[serde(default, skip_serializing_if = "TraceContext::is_empty")]
    pub trace_context: TraceContext,
}

/// Resume package loaded from a durable session store.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SessionResumeSnapshot {
    /// Session record.
    pub session: SessionRecord,
    /// Latest checkpoint for the requested run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_checkpoint: Option<AgentCheckpoint>,
    /// Replayable stream records after the checkpoint cursor.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stream_records: Vec<AgentStreamRecord>,
}

/// Session store failure.
#[derive(Debug, Error)]
pub enum SessionStoreError {
    /// Record was not found.
    #[error("session record not found: {0}")]
    NotFound(String),
    /// Store failed.
    #[error("session store failed: {0}")]
    Failed(String),
}

/// Result alias for session store operations.
pub type SessionStoreResult<T> = Result<T, SessionStoreError>;

impl From<SessionStoreError> for AgentExecutorError {
    fn from(error: SessionStoreError) -> Self {
        Self::Failed(error.to_string())
    }
}

/// Durable session store contract.
#[async_trait]
pub trait SessionStore: Send + Sync {
    /// Save a session record.
    async fn save_session(&self, session: SessionRecord) -> SessionStoreResult<()>;

    /// Load a session record.
    async fn load_session(&self, session_id: &SessionId) -> SessionStoreResult<SessionRecord>;

    /// Append a run record.
    async fn append_run(&self, run: RunRecord) -> SessionStoreResult<()>;

    /// Append a checkpoint.
    async fn append_checkpoint(
        &self,
        session_id: &SessionId,
        checkpoint: AgentCheckpoint,
    ) -> SessionStoreResult<()>;

    /// Append stream records.
    async fn append_stream_records(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        records: Vec<AgentStreamRecord>,
    ) -> SessionStoreResult<()>;

    /// Replay stream records for a run.
    async fn replay_stream_records(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<Vec<AgentStreamRecord>>;

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

    /// Replay stream records after a cursor.
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

    /// Load a resume snapshot from session, checkpoint, and stream evidence.
    async fn resume_snapshot(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<SessionResumeSnapshot> {
        let session = self.load_session(session_id).await?;
        let latest_checkpoint = self.latest_checkpoint(session_id, run_id).await?;
        let stream_records = self
            .replay_stream_records_after(
                session_id,
                run_id,
                latest_checkpoint
                    .as_ref()
                    .and_then(|checkpoint| checkpoint.resume.cursor.stream_cursor),
            )
            .await?;
        Ok(SessionResumeSnapshot {
            session,
            latest_checkpoint,
            stream_records,
        })
    }

    /// Return compact run trace projection.
    async fn compact_run_trace(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<CompactRunTrace>;
}

/// In-memory session store for tests and single-process services.
#[derive(Clone, Debug, Default)]
pub struct InMemorySessionStore {
    inner: Arc<Mutex<StoreInner>>,
}

#[derive(Clone, Debug, Default)]
struct StoreInner {
    sessions: BTreeMap<SessionId, SessionRecord>,
    runs: BTreeMap<String, RunRecord>,
    checkpoints: BTreeMap<String, Vec<AgentCheckpoint>>,
    streams: BTreeMap<String, Vec<AgentStreamRecord>>,
}

impl InMemorySessionStore {
    /// Create an empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

fn run_key(session_id: &SessionId, run_id: &RunId) -> String {
    format!("{}:{}", session_id.as_str(), run_id.as_str())
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
impl AgentExecutor for SessionStoreExecutor {
    async fn checkpoint(
        &self,
        checkpoint: AgentCheckpoint,
    ) -> Result<AgentExecutionDecision, AgentExecutorError> {
        self.store
            .append_checkpoint(&self.session_id, checkpoint)
            .await?;
        Ok(AgentExecutionDecision::Continue)
    }
}

#[async_trait]
impl SessionStore for InMemorySessionStore {
    async fn save_session(&self, session: SessionRecord) -> SessionStoreResult<()> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|error| SessionStoreError::Failed(error.to_string()))?;
        inner.sessions.insert(session.session_id.clone(), session);
        Ok(())
    }

    async fn load_session(&self, session_id: &SessionId) -> SessionStoreResult<SessionRecord> {
        let inner = self
            .inner
            .lock()
            .map_err(|error| SessionStoreError::Failed(error.to_string()))?;
        inner
            .sessions
            .get(session_id)
            .cloned()
            .ok_or_else(|| SessionStoreError::NotFound(session_id.as_str().to_string()))
    }

    async fn append_run(&self, run: RunRecord) -> SessionStoreResult<()> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|error| SessionStoreError::Failed(error.to_string()))?;
        inner
            .runs
            .insert(run_key(&run.session_id, &run.run_id), run);
        Ok(())
    }

    async fn append_checkpoint(
        &self,
        session_id: &SessionId,
        checkpoint: AgentCheckpoint,
    ) -> SessionStoreResult<()> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|error| SessionStoreError::Failed(error.to_string()))?;
        inner
            .checkpoints
            .entry(run_key(session_id, &checkpoint.run_id))
            .or_default()
            .push(checkpoint);
        Ok(())
    }

    async fn append_stream_records(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        records: Vec<AgentStreamRecord>,
    ) -> SessionStoreResult<()> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|error| SessionStoreError::Failed(error.to_string()))?;
        let stream = inner
            .streams
            .entry(run_key(session_id, run_id))
            .or_default();
        for record in records {
            if stream
                .last()
                .map_or(true, |last| record.sequence > last.sequence)
            {
                stream.push(record);
            }
        }
        Ok(())
    }

    async fn replay_stream_records(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<Vec<AgentStreamRecord>> {
        let inner = self
            .inner
            .lock()
            .map_err(|error| SessionStoreError::Failed(error.to_string()))?;
        Ok(inner
            .streams
            .get(&run_key(session_id, run_id))
            .cloned()
            .unwrap_or_default())
    }

    async fn load_checkpoints(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<Vec<AgentCheckpoint>> {
        let inner = self
            .inner
            .lock()
            .map_err(|error| SessionStoreError::Failed(error.to_string()))?;
        Ok(inner
            .checkpoints
            .get(&run_key(session_id, run_id))
            .cloned()
            .unwrap_or_default())
    }

    async fn compact_run_trace(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<CompactRunTrace> {
        let inner = self
            .inner
            .lock()
            .map_err(|error| SessionStoreError::Failed(error.to_string()))?;
        let key = run_key(session_id, run_id);
        let run = inner
            .runs
            .get(&key)
            .ok_or_else(|| SessionStoreError::NotFound(key.clone()))?;
        let checkpoints = inner.checkpoints.get(&key).cloned().unwrap_or_default();
        let streams = inner.streams.get(&key).cloned().unwrap_or_default();
        Ok(CompactRunTrace {
            run_id: Some(run_id.clone()),
            checkpoints: checkpoints
                .iter()
                .map(|checkpoint| checkpoint.checkpoint_id.clone())
                .collect(),
            stream_events: streams.len(),
            latest_checkpoint: checkpoints
                .last()
                .map(|checkpoint| checkpoint.checkpoint_id.clone()),
            stream_cursor: streams.last().map(|record| record.sequence),
            trace_context: run.trace_context.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use starweaver_context::AgentContext;
    use starweaver_runtime::{
        Agent, AgentCheckpoint, AgentExecutionNode, AgentRunState, AgentStreamEvent,
        AgentStreamRecord,
    };

    #[tokio::test]
    async fn in_memory_store_saves_session_and_run_projection() {
        let store = InMemorySessionStore::new();
        let session_id = SessionId::from_string("session-1");
        let context = AgentContext::default();
        let run_id = RunId::new();
        let conversation_id = ConversationId::new();
        store
            .save_session(SessionRecord {
                session_id: session_id.clone(),
                state: context.export_state(),
                trace_context: TraceContext::from_trace_id("trace-1"),
                metadata: Metadata::default(),
            })
            .await
            .unwrap();
        store
            .append_run(RunRecord {
                session_id: session_id.clone(),
                run_id: run_id.clone(),
                conversation_id: conversation_id.clone(),
                trace_context: TraceContext::from_trace_id("trace-1"),
                metadata: Metadata::default(),
            })
            .await
            .unwrap();
        let mut run_state = AgentRunState::new(run_id.clone(), conversation_id);
        run_state.run_step = 1;
        let checkpoint = AgentCheckpoint::new(AgentExecutionNode::ModelResponse, &run_state);
        let checkpoint_id = checkpoint.checkpoint_id.clone();
        store
            .append_checkpoint(&session_id, checkpoint)
            .await
            .unwrap();
        store
            .append_stream_records(
                &session_id,
                &run_id,
                vec![AgentStreamRecord::new(
                    0,
                    AgentStreamEvent::ModelRequest { step: 0 },
                )],
            )
            .await
            .unwrap();

        let replay = store
            .replay_stream_records(&session_id, &run_id)
            .await
            .unwrap();
        let trace = store.compact_run_trace(&session_id, &run_id).await.unwrap();
        assert_eq!(
            store.load_session(&session_id).await.unwrap().session_id,
            session_id
        );
        assert_eq!(replay.len(), 1);
        assert_eq!(trace.run_id.as_ref(), Some(&run_id));
        assert_eq!(trace.checkpoints, vec![checkpoint_id.clone()]);
        assert_eq!(trace.latest_checkpoint, Some(checkpoint_id));
        assert_eq!(trace.stream_events, 1);
        assert_eq!(trace.stream_cursor, Some(0));
        assert_eq!(trace.trace_context.trace_id.as_deref(), Some("trace-1"));
        assert!(store
            .latest_checkpoint(&session_id, &run_id)
            .await
            .unwrap()
            .is_some());
        assert!(store
            .resume_snapshot(&session_id, &run_id)
            .await
            .unwrap()
            .latest_checkpoint
            .is_some());
    }

    #[tokio::test]
    async fn store_scopes_run_data_by_session() {
        let store = InMemorySessionStore::new();
        let session_one = SessionId::from_string("session-one");
        let session_two = SessionId::from_string("session-two");
        let run_id = RunId::new();
        let conversation_id = ConversationId::new();
        store
            .append_run(RunRecord {
                session_id: session_one.clone(),
                run_id: run_id.clone(),
                conversation_id: conversation_id.clone(),
                trace_context: TraceContext::from_trace_id("trace-one"),
                metadata: Metadata::default(),
            })
            .await
            .unwrap();
        store
            .append_run(RunRecord {
                session_id: session_two.clone(),
                run_id: run_id.clone(),
                conversation_id,
                trace_context: TraceContext::from_trace_id("trace-two"),
                metadata: Metadata::default(),
            })
            .await
            .unwrap();
        store
            .append_stream_records(
                &session_one,
                &run_id,
                vec![AgentStreamRecord::new(
                    0,
                    AgentStreamEvent::ModelRequest { step: 0 },
                )],
            )
            .await
            .unwrap();
        store
            .append_stream_records(
                &session_two,
                &run_id,
                vec![AgentStreamRecord::new(
                    1,
                    AgentStreamEvent::ModelRequest { step: 1 },
                )],
            )
            .await
            .unwrap();

        let one_trace = store
            .compact_run_trace(&session_one, &run_id)
            .await
            .unwrap();
        let two_trace = store
            .compact_run_trace(&session_two, &run_id)
            .await
            .unwrap();
        assert_eq!(
            one_trace.trace_context.trace_id.as_deref(),
            Some("trace-one")
        );
        assert_eq!(
            two_trace.trace_context.trace_id.as_deref(),
            Some("trace-two")
        );
        assert_eq!(
            store
                .replay_stream_records(&session_one, &run_id)
                .await
                .unwrap()[0]
                .sequence,
            0
        );
        assert_eq!(
            store
                .replay_stream_records(&session_two, &run_id)
                .await
                .unwrap()[0]
                .sequence,
            1
        );
    }

    #[tokio::test]
    async fn stream_replay_after_cursor_returns_unpersisted_tail() {
        let store = InMemorySessionStore::new();
        let session_id = SessionId::from_string("session-stream");
        let run_id = RunId::new();
        store
            .append_stream_records(
                &session_id,
                &run_id,
                vec![
                    AgentStreamRecord::new(0, AgentStreamEvent::ModelRequest { step: 0 }),
                    AgentStreamRecord::new(
                        1,
                        AgentStreamEvent::Checkpoint {
                            node: AgentExecutionNode::ModelResponse,
                            step: 1,
                        },
                    ),
                ],
            )
            .await
            .unwrap();

        let tail = store
            .replay_stream_records_after(&session_id, &run_id, Some(0))
            .await
            .unwrap();
        assert_eq!(tail.len(), 1);
        assert_eq!(tail[0].sequence, 1);
    }

    #[tokio::test]
    async fn session_store_executor_persists_runtime_checkpoints() {
        let store = Arc::new(InMemorySessionStore::new());
        let session_id = SessionId::from_string("session-executor");
        let executor = Arc::new(SessionStoreExecutor::new(store.clone(), session_id.clone()));
        let agent = Agent::new(Arc::new(starweaver_model::TestModel::with_text("ok")))
            .with_executor(executor);

        let result = agent.run("hello").await.unwrap();
        store
            .append_run(RunRecord {
                session_id: session_id.clone(),
                run_id: result.state.run_id.clone(),
                conversation_id: result.state.conversation_id.clone(),
                trace_context: TraceContext::default(),
                metadata: Metadata::default(),
            })
            .await
            .unwrap();

        let trace = store
            .compact_run_trace(&session_id, &result.state.run_id)
            .await
            .unwrap();
        assert!(trace.checkpoints.len() >= 4);
    }
}
