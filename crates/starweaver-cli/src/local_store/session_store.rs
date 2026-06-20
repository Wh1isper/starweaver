//! `SessionStore` adapter over the CLI local store.

use async_trait::async_trait;
use chrono::Utc;
use rusqlite::{params, OptionalExtension, TransactionBehavior};
use starweaver_context::ResumableState;
use starweaver_core::{CheckpointId, RunId, SessionId};
use starweaver_runtime::{AgentCheckpoint, AgentStreamRecord};
use starweaver_session::{
    ApprovalRecord, CheckpointRef, CompactRunTrace, CompactSessionTrace, DeferredToolRecord,
    EnvironmentStateRef, RunRecord, RunStatus, SessionFilter, SessionRecord, SessionStatus,
    SessionStore, SessionStoreError, SessionStoreResult, StreamCursorRef,
};

use super::{
    db::{
        insert_approval_records_tx, insert_deferred_tool_records_tx, insert_raw_stream_records_tx,
        insert_stream_cursor_tx, load_session_tx, next_sequence_tx, upsert_run_tx,
        upsert_session_tx,
    },
    LocalStore,
};
use crate::{config::CliConfig, CliError};

/// Shared session store adapter backed by the CLI local `SQLite` store.
#[derive(Clone, Debug)]
pub struct LocalSessionStore {
    config: CliConfig,
}

impl LocalSessionStore {
    /// Create a local session store adapter from resolved CLI config.
    #[must_use]
    pub const fn new(config: CliConfig) -> Self {
        Self { config }
    }

    fn open_store(&self) -> SessionStoreResult<LocalStore> {
        LocalStore::open(&self.config).map_err(session_failed_cli)
    }
}

#[async_trait]
impl SessionStore for LocalSessionStore {
    async fn save_session(&self, mut session: SessionRecord) -> SessionStoreResult<()> {
        session.updated_at = Utc::now();
        let mut store = self.open_store()?;
        let tx = store
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(session_failed)?;
        upsert_session_tx(&tx, &session).map_err(session_failed)?;
        tx.commit().map_err(session_failed)
    }

    async fn load_session(&self, session_id: &SessionId) -> SessionStoreResult<SessionRecord> {
        self.open_store()?
            .load_session(session_id.as_str())
            .map_err(session_failed_cli)
    }

    async fn list_sessions(&self, filter: SessionFilter) -> SessionStoreResult<Vec<SessionRecord>> {
        let store = self.open_store()?;
        let mut stmt = store
            .conn
            .prepare("SELECT record_json FROM sessions ORDER BY updated_at DESC")
            .map_err(session_failed)?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(session_failed)?;
        let mut sessions = Vec::new();
        for row in rows {
            let session: SessionRecord =
                serde_json::from_str(&row.map_err(session_failed)?).map_err(session_failed)?;
            if filter.status.is_some_and(|status| session.status != status) {
                continue;
            }
            if filter
                .profile
                .as_ref()
                .is_some_and(|profile| session.profile.as_ref() != Some(profile))
            {
                continue;
            }
            if filter
                .workspace
                .as_ref()
                .is_some_and(|workspace| session.workspace.as_ref() != Some(workspace))
            {
                continue;
            }
            sessions.push(session);
            if filter.limit.is_some_and(|limit| sessions.len() >= limit) {
                break;
            }
        }
        Ok(sessions)
    }

    async fn update_session_status(
        &self,
        session_id: &SessionId,
        status: SessionStatus,
    ) -> SessionStoreResult<()> {
        let mut session = self.load_session(session_id).await?;
        session.status = status;
        self.save_session(session).await
    }

    async fn save_context_state(
        &self,
        session_id: &SessionId,
        state: ResumableState,
    ) -> SessionStoreResult<()> {
        let mut session = self.load_session(session_id).await?;
        session.state = state;
        self.save_session(session).await
    }

    async fn save_environment_state(
        &self,
        session_id: &SessionId,
        environment_state: EnvironmentStateRef,
    ) -> SessionStoreResult<()> {
        let mut session = self.load_session(session_id).await?;
        session.environment_state = Some(environment_state);
        self.save_session(session).await
    }

    async fn append_run(&self, mut run: RunRecord) -> SessionStoreResult<()> {
        let mut store = self.open_store()?;
        let tx = store
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(session_failed)?;
        let mut session = load_session_tx(&tx, run.session_id.as_str()).map_err(session_failed)?;
        run.updated_at = Utc::now();
        if let Some(existing_sequence) =
            existing_run_sequence(&tx, run.session_id.as_str(), run.run_id.as_str())?
        {
            run.sequence_no = existing_sequence;
        } else if run.sequence_no == 0
            || sequence_exists(&tx, run.session_id.as_str(), run.sequence_no)?
        {
            run.sequence_no =
                next_sequence_tx(&tx, run.session_id.as_str()).map_err(session_failed)?;
        }
        apply_run_to_session(&mut session, &run);
        upsert_run_tx(&tx, &run).map_err(session_failed)?;
        upsert_session_tx(&tx, &session).map_err(session_failed)?;
        tx.commit().map_err(session_failed)
    }

    async fn load_run(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<RunRecord> {
        self.open_store()?
            .load_run(session_id.as_str(), run_id.as_str())
            .map_err(session_failed_cli)
    }

    async fn list_runs(&self, session_id: &SessionId) -> SessionStoreResult<Vec<RunRecord>> {
        let store = self.open_store()?;
        let mut stmt = store
            .conn
            .prepare("SELECT record_json FROM runs WHERE session_id = ?1 ORDER BY sequence_no ASC")
            .map_err(session_failed)?;
        let rows = stmt
            .query_map(params![session_id.as_str()], |row| row.get::<_, String>(0))
            .map_err(session_failed)?;
        let runs = collect_json_records(rows)?;
        Ok(runs)
    }

    async fn update_run_status(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        status: RunStatus,
        output_preview: Option<String>,
    ) -> SessionStoreResult<()> {
        let mut run = self.load_run(session_id, run_id).await?;
        run.status = status;
        run.output_preview = output_preview;
        run.updated_at = Utc::now();
        self.append_run(run).await
    }

    async fn append_checkpoint(
        &self,
        session_id: &SessionId,
        checkpoint: AgentCheckpoint,
    ) -> SessionStoreResult<()> {
        let mut store = self.open_store()?;
        let tx = store
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(session_failed)?;
        let checkpoint_id = checkpoint.checkpoint_id.clone();
        let checkpoint_run_id = checkpoint.run_id.clone();
        let checkpoint_node = checkpoint.node;
        let checkpoint_node_label = format!("{checkpoint_node:?}");
        let checkpoint_sequence = checkpoint.run_step;
        let stream_cursor = checkpoint.resume.cursor.stream_cursor;
        let checkpoint_metadata = checkpoint.metadata.clone();
        let mut run = load_run_tx(&tx, session_id.as_str(), checkpoint_run_id.as_str())?;
        tx.execute(
            "INSERT OR REPLACE INTO checkpoints
             (checkpoint_id, session_id, run_id, sequence_no, node, checkpoint_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                checkpoint_id.as_str(),
                session_id.as_str(),
                checkpoint_run_id.as_str(),
                i64::try_from(checkpoint_sequence).map_err(session_failed)?,
                checkpoint_node_label,
                serde_json::to_string(&checkpoint).map_err(session_failed)?,
                Utc::now().to_rfc3339(),
            ],
        )
        .map_err(session_failed)?;
        run.latest_checkpoint = Some(CheckpointRef {
            checkpoint_id,
            run_id: checkpoint_run_id,
            sequence: checkpoint_sequence,
            node: format!("{checkpoint_node:?}"),
            storage_ref: None,
            stream_cursor,
            created_at: Utc::now(),
            metadata: checkpoint_metadata,
        });
        run.updated_at = Utc::now();
        upsert_run_tx(&tx, &run).map_err(session_failed)?;
        tx.commit().map_err(session_failed)
    }

    async fn load_checkpoints(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<Vec<AgentCheckpoint>> {
        let store = self.open_store()?;
        let mut stmt = store
            .conn
            .prepare(
                "SELECT checkpoint_json FROM checkpoints
                 WHERE session_id = ?1 AND run_id = ?2
                 ORDER BY sequence_no ASC, checkpoint_id ASC",
            )
            .map_err(session_failed)?;
        let rows = stmt
            .query_map(params![session_id.as_str(), run_id.as_str()], |row| {
                row.get::<_, String>(0)
            })
            .map_err(session_failed)?;
        let mut checkpoints = Vec::new();
        for row in rows {
            let json = row.map_err(session_failed)?;
            if let Ok(checkpoint) = serde_json::from_str::<AgentCheckpoint>(&json) {
                checkpoints.push(checkpoint);
            }
        }
        Ok(checkpoints)
    }

    async fn append_stream_records(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        records: Vec<AgentStreamRecord>,
    ) -> SessionStoreResult<()> {
        let mut store = self.open_store()?;
        let tx = store
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(session_failed)?;
        let mut run = load_run_tx(&tx, session_id.as_str(), run_id.as_str())?;
        insert_raw_stream_records_tx(&tx, &run, &records).map_err(session_failed)?;
        if let Some(sequence) = latest_raw_sequence(&tx, session_id.as_str(), run_id.as_str())? {
            let cursor =
                StreamCursorRef::new("raw_runtime", format!("run:{}", run_id.as_str()), sequence);
            run.stream_cursors
                .retain(|existing| existing.family != cursor.family);
            run.stream_cursors.push(cursor.clone());
            run.updated_at = Utc::now();
            upsert_run_tx(&tx, &run).map_err(session_failed)?;
            let mut session = load_session_tx(&tx, session_id.as_str()).map_err(session_failed)?;
            upsert_session_cursor(&mut session, cursor);
            upsert_session_tx(&tx, &session).map_err(session_failed)?;
        }
        tx.commit().map_err(session_failed)
    }

    async fn replay_stream_records(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<Vec<AgentStreamRecord>> {
        self.replay_stream_records_after(session_id, run_id, None)
            .await
    }

    async fn replay_stream_records_after(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        after_sequence: Option<usize>,
    ) -> SessionStoreResult<Vec<AgentStreamRecord>> {
        let after = after_sequence.map_or(-1_i64, |value| i64::try_from(value).unwrap_or(i64::MAX));
        let store = self.open_store()?;
        let mut stmt = store
            .conn
            .prepare(
                "SELECT record_json FROM raw_stream_records
                 WHERE session_id = ?1 AND run_id = ?2 AND sequence_no > ?3
                 ORDER BY sequence_no ASC",
            )
            .map_err(session_failed)?;
        let rows = stmt
            .query_map(
                params![session_id.as_str(), run_id.as_str(), after],
                |row| row.get::<_, String>(0),
            )
            .map_err(session_failed)?;
        let records = collect_json_records(rows)?;
        Ok(records)
    }

    async fn save_stream_cursor(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        cursor: StreamCursorRef,
    ) -> SessionStoreResult<()> {
        let mut store = self.open_store()?;
        let tx = store
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(session_failed)?;
        let mut run = load_run_tx(&tx, session_id.as_str(), run_id.as_str())?;
        run.stream_cursors
            .retain(|existing| existing.family != cursor.family || existing.scope != cursor.scope);
        run.stream_cursors.push(cursor.clone());
        run.updated_at = Utc::now();
        upsert_run_tx(&tx, &run).map_err(session_failed)?;
        let mut session = load_session_tx(&tx, session_id.as_str()).map_err(session_failed)?;
        upsert_session_cursor(&mut session, cursor.clone());
        upsert_session_tx(&tx, &session).map_err(session_failed)?;
        insert_stream_cursor_tx(&tx, &run, &cursor).map_err(session_failed)?;
        tx.commit().map_err(session_failed)
    }

    async fn append_approval(&self, approval: ApprovalRecord) -> SessionStoreResult<()> {
        let mut store = self.open_store()?;
        let tx = store
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(session_failed)?;
        insert_approval_records_tx(&tx, &[approval]).map_err(session_failed)?;
        tx.commit().map_err(session_failed)
    }

    async fn load_approvals(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<Vec<ApprovalRecord>> {
        self.open_store()?
            .list_approvals(Some(session_id.as_str()), Some(run_id.as_str()))
            .map_err(session_failed_cli)
    }

    async fn append_deferred_tool(&self, record: DeferredToolRecord) -> SessionStoreResult<()> {
        let mut store = self.open_store()?;
        let tx = store
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(session_failed)?;
        insert_deferred_tool_records_tx(&tx, &[record]).map_err(session_failed)?;
        tx.commit().map_err(session_failed)
    }

    async fn load_deferred_tools(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<Vec<DeferredToolRecord>> {
        self.open_store()?
            .list_deferred_tools(Some(session_id.as_str()), Some(run_id.as_str()))
            .map_err(session_failed_cli)
    }

    async fn compact_run_trace(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<CompactRunTrace> {
        let store = self.open_store()?;
        let run = store
            .load_run(session_id.as_str(), run_id.as_str())
            .map_err(session_failed_cli)?;
        Ok(CompactRunTrace {
            session_id: Some(session_id.clone()),
            run_id: Some(run_id.clone()),
            status: run.status,
            checkpoints: checkpoint_ids(&store, session_id.as_str(), run_id.as_str())?,
            approvals: pending_approval_count(&store, session_id.as_str(), run_id.as_str())?,
            deferred_tools: pending_deferred_count(&store, session_id.as_str(), run_id.as_str())?,
            latest_checkpoint: run
                .latest_checkpoint
                .as_ref()
                .map(|checkpoint| checkpoint.checkpoint_id.clone()),
            stream_cursor: latest_raw_sequence_ref(&store, session_id.as_str(), run_id.as_str())?,
            stream_cursors: run.stream_cursors,
            output_preview: run.output_preview,
            trace_context: run.trace_context,
            updated_at: Some(run.updated_at),
            metadata: run.metadata,
        })
    }

    async fn compact_session_trace(
        &self,
        session_id: &SessionId,
    ) -> SessionStoreResult<CompactSessionTrace> {
        let session = self.load_session(session_id).await?;
        let runs = self.list_runs(session_id).await?;
        let latest_run = runs.last();
        Ok(CompactSessionTrace {
            session_id: session.session_id,
            title: session.title,
            workspace: session.workspace,
            profile: session.profile,
            status: session.status,
            runs: runs.len(),
            latest_run_id: latest_run.map(|run| run.run_id.clone()),
            last_output_preview: latest_run.and_then(|run| run.output_preview.clone()),
            stream_cursors: session.stream_cursors,
            trace_context: session.trace_context,
            created_at: session.created_at,
            updated_at: session.updated_at,
            metadata: session.metadata,
        })
    }
}

fn load_run_tx(
    tx: &rusqlite::Transaction<'_>,
    session_id: &str,
    run_id: &str,
) -> SessionStoreResult<RunRecord> {
    tx.query_row(
        "SELECT record_json FROM runs WHERE session_id = ?1 AND run_id = ?2",
        params![session_id, run_id],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .map_err(session_failed)?
    .map(|json| serde_json::from_str(&json).map_err(session_failed))
    .transpose()?
    .ok_or_else(|| SessionStoreError::NotFound(format!("{session_id}:{run_id}")))
}

fn existing_run_sequence(
    tx: &rusqlite::Transaction<'_>,
    session_id: &str,
    run_id: &str,
) -> SessionStoreResult<Option<usize>> {
    tx.query_row(
        "SELECT sequence_no FROM runs WHERE session_id = ?1 AND run_id = ?2",
        params![session_id, run_id],
        |row| row.get::<_, i64>(0),
    )
    .optional()
    .map_err(session_failed)?
    .map(|value| usize::try_from(value).map_err(session_failed))
    .transpose()
}

fn sequence_exists(
    tx: &rusqlite::Transaction<'_>,
    session_id: &str,
    sequence_no: usize,
) -> SessionStoreResult<bool> {
    let count = tx
        .query_row(
            "SELECT COUNT(*) FROM runs WHERE session_id = ?1 AND sequence_no = ?2",
            params![
                session_id,
                i64::try_from(sequence_no).map_err(session_failed)?
            ],
            |row| row.get::<_, i64>(0),
        )
        .map_err(session_failed)?;
    Ok(count > 0)
}

fn apply_run_to_session(session: &mut SessionRecord, run: &RunRecord) {
    session.profile.clone_from(&run.profile);
    session.head_run_id = Some(run.run_id.clone());
    if run.status == RunStatus::Completed {
        session.head_success_run_id = Some(run.run_id.clone());
    }
    if matches!(
        run.status,
        RunStatus::Queued | RunStatus::Running | RunStatus::Waiting
    ) {
        session.active_run_id = Some(run.run_id.clone());
    } else if session.active_run_id.as_ref() == Some(&run.run_id) {
        session.active_run_id = None;
    }
    session.updated_at = run.updated_at;
}

fn upsert_session_cursor(session: &mut SessionRecord, cursor: StreamCursorRef) {
    session
        .stream_cursors
        .retain(|existing| existing.family != cursor.family || existing.scope != cursor.scope);
    session.stream_cursors.push(cursor);
    session.updated_at = Utc::now();
}

fn latest_raw_sequence(
    tx: &rusqlite::Transaction<'_>,
    session_id: &str,
    run_id: &str,
) -> SessionStoreResult<Option<usize>> {
    tx.query_row(
        "SELECT MAX(sequence_no) FROM raw_stream_records WHERE session_id = ?1 AND run_id = ?2",
        params![session_id, run_id],
        |row| row.get::<_, Option<i64>>(0),
    )
    .map_err(session_failed)?
    .map(|value| usize::try_from(value).map_err(session_failed))
    .transpose()
}

fn latest_raw_sequence_ref(
    store: &LocalStore,
    session_id: &str,
    run_id: &str,
) -> SessionStoreResult<Option<usize>> {
    store
        .conn
        .query_row(
            "SELECT MAX(sequence_no) FROM raw_stream_records WHERE session_id = ?1 AND run_id = ?2",
            params![session_id, run_id],
            |row| row.get::<_, Option<i64>>(0),
        )
        .map_err(session_failed)?
        .map(|value| usize::try_from(value).map_err(session_failed))
        .transpose()
}

fn checkpoint_ids(
    store: &LocalStore,
    session_id: &str,
    run_id: &str,
) -> SessionStoreResult<Vec<CheckpointId>> {
    let mut stmt = store
        .conn
        .prepare(
            "SELECT checkpoint_id FROM checkpoints
             WHERE session_id = ?1 AND run_id = ?2
             ORDER BY sequence_no ASC, checkpoint_id ASC",
        )
        .map_err(session_failed)?;
    let rows = stmt
        .query_map(params![session_id, run_id], |row| row.get::<_, String>(0))
        .map_err(session_failed)?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(session_failed)
        .map(|ids| ids.into_iter().map(CheckpointId::from_string).collect())
}

fn pending_approval_count(
    store: &LocalStore,
    session_id: &str,
    run_id: &str,
) -> SessionStoreResult<usize> {
    count_rows(
        store,
        "SELECT COUNT(*) FROM approvals WHERE session_id = ?1 AND run_id = ?2 AND status = 'pending'",
        session_id,
        run_id,
    )
}

fn pending_deferred_count(
    store: &LocalStore,
    session_id: &str,
    run_id: &str,
) -> SessionStoreResult<usize> {
    count_rows(
        store,
        "SELECT COUNT(*) FROM deferred_tools
         WHERE session_id = ?1 AND run_id = ?2
           AND status IN ('pending', 'running', 'waiting')",
        session_id,
        run_id,
    )
}

fn count_rows(
    store: &LocalStore,
    sql: &str,
    session_id: &str,
    run_id: &str,
) -> SessionStoreResult<usize> {
    let count = store
        .conn
        .query_row(sql, params![session_id, run_id], |row| row.get::<_, i64>(0))
        .map_err(session_failed)?;
    usize::try_from(count).map_err(session_failed)
}

fn collect_json_records<T>(
    rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<String>>,
) -> SessionStoreResult<Vec<T>>
where
    T: serde::de::DeserializeOwned,
{
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(session_failed)?
        .into_iter()
        .map(|json| serde_json::from_str(&json).map_err(session_failed))
        .collect()
}

fn session_failed(error: impl std::fmt::Display) -> SessionStoreError {
    SessionStoreError::Failed(error.to_string())
}

fn session_failed_cli(error: CliError) -> SessionStoreError {
    match error {
        CliError::NotFound(id) => SessionStoreError::NotFound(id),
        error => SessionStoreError::Failed(error.to_string()),
    }
}
