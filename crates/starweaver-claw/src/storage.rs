//! SQLite storage adapters for Claw.

use std::{
    path::Path,
    sync::{Arc, Mutex, MutexGuard},
};

use async_trait::async_trait;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use starweaver_context::ResumableState;
use starweaver_core::{Metadata, RunId, SessionId};
use starweaver_runtime::{AgentCheckpoint, AgentStreamRecord};
use starweaver_session::{
    ApprovalRecord, ApprovalStatus, CompactRunTrace, CompactSessionTrace, DeferredToolRecord,
    EnvironmentStateRef, RunRecord, RunStatus, SessionFilter, SessionRecord, SessionResumeSnapshot,
    SessionStatus, SessionStore, SessionStoreError, SessionStoreResult, StreamCursorRef,
};
use starweaver_stream::{
    InMemoryReplayEventLog, ReplayCursor, ReplayError, ReplayEvent, ReplayEventKind,
    ReplayEventLog, ReplayResult, ReplayScope, ReplaySnapshot, ReplaySubscription,
};

/// SQLite-backed durable session store.
#[derive(Clone, Debug)]
pub struct SqliteSessionStore {
    connection: Arc<Mutex<Connection>>,
}

impl SqliteSessionStore {
    /// Open or create a SQLite session store.
    ///
    /// # Errors
    ///
    /// Returns a store error when SQLite cannot open or initialize the database.
    pub fn open(path: impl AsRef<Path>) -> SessionStoreResult<Self> {
        let connection = Connection::open(path).map_err(sql_error)?;
        let store = Self {
            connection: Arc::new(Mutex::new(connection)),
        };
        store.migrate()?;
        Ok(store)
    }

    /// Open an in-memory SQLite session store.
    ///
    /// # Errors
    ///
    /// Returns a store error when SQLite cannot initialize the database.
    pub fn in_memory() -> SessionStoreResult<Self> {
        let connection = Connection::open_in_memory().map_err(sql_error)?;
        let store = Self {
            connection: Arc::new(Mutex::new(connection)),
        };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> SessionStoreResult<()> {
        let connection = self.lock()?;
        connection
            .execute_batch(
                r"
                PRAGMA journal_mode = WAL;
                PRAGMA foreign_keys = ON;

                CREATE TABLE IF NOT EXISTS sessions (
                    session_id TEXT PRIMARY KEY,
                    record TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS runs (
                    session_id TEXT NOT NULL,
                    run_id TEXT NOT NULL,
                    record TEXT NOT NULL,
                    sequence_no INTEGER NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    PRIMARY KEY (session_id, run_id)
                );
                CREATE INDEX IF NOT EXISTS ix_claw_runs_session_sequence
                    ON runs(session_id, sequence_no);

                CREATE TABLE IF NOT EXISTS checkpoints (
                    session_id TEXT NOT NULL,
                    run_id TEXT NOT NULL,
                    sequence_no INTEGER NOT NULL,
                    checkpoint_id TEXT NOT NULL,
                    record TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    PRIMARY KEY (session_id, run_id, sequence_no, checkpoint_id)
                );

                CREATE TABLE IF NOT EXISTS stream_records (
                    session_id TEXT NOT NULL,
                    run_id TEXT NOT NULL,
                    sequence_no INTEGER NOT NULL,
                    record TEXT NOT NULL,
                    PRIMARY KEY (session_id, run_id, sequence_no)
                );

                CREATE TABLE IF NOT EXISTS approvals (
                    session_id TEXT NOT NULL,
                    run_id TEXT NOT NULL,
                    approval_id TEXT NOT NULL,
                    record TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    PRIMARY KEY (session_id, run_id, approval_id)
                );

                CREATE TABLE IF NOT EXISTS deferred_tools (
                    session_id TEXT NOT NULL,
                    run_id TEXT NOT NULL,
                    deferred_id TEXT NOT NULL,
                    record TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    PRIMARY KEY (session_id, run_id, deferred_id)
                );

                CREATE TABLE IF NOT EXISTS replay_events (
                    scope TEXT NOT NULL,
                    sequence_no INTEGER NOT NULL,
                    record TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    PRIMARY KEY (scope, sequence_no)
                );
                CREATE INDEX IF NOT EXISTS ix_replay_events_scope_sequence
                    ON replay_events(scope, sequence_no);

                CREATE TABLE IF NOT EXISTS replay_snapshots (
                    scope TEXT PRIMARY KEY,
                    record TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );
                ",
            )
            .map_err(sql_error)?;
        Ok(())
    }

    fn lock(&self) -> SessionStoreResult<MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|error| SessionStoreError::Failed(error.to_string()))
    }
}

#[async_trait]
impl SessionStore for SqliteSessionStore {
    async fn save_session(&self, mut session: SessionRecord) -> SessionStoreResult<()> {
        session.updated_at = Utc::now();
        let connection = self.lock()?;
        save_session_record(&connection, &session)
    }

    async fn load_session(&self, session_id: &SessionId) -> SessionStoreResult<SessionRecord> {
        let connection = self.lock()?;
        load_session_record(&connection, session_id)
    }

    async fn list_sessions(&self, filter: SessionFilter) -> SessionStoreResult<Vec<SessionRecord>> {
        let connection = self.lock()?;
        let mut statement = connection
            .prepare("SELECT record FROM sessions ORDER BY updated_at DESC")
            .map_err(sql_error)?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(sql_error)?;
        let mut sessions = Vec::new();
        for row in rows {
            let session = deserialize::<SessionRecord>(&row.map_err(sql_error)?)?;
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
        let connection = self.lock()?;
        let mut session = load_session_record(&connection, session_id)?;
        session.status = status;
        session.updated_at = Utc::now();
        save_session_record(&connection, &session)
    }

    async fn save_context_state(
        &self,
        session_id: &SessionId,
        state: ResumableState,
    ) -> SessionStoreResult<()> {
        let connection = self.lock()?;
        let mut session = load_session_record(&connection, session_id)?;
        session.state = state;
        session.updated_at = Utc::now();
        save_session_record(&connection, &session)
    }

    async fn save_environment_state(
        &self,
        session_id: &SessionId,
        environment_state: EnvironmentStateRef,
    ) -> SessionStoreResult<()> {
        let connection = self.lock()?;
        let mut session = load_session_record(&connection, session_id)?;
        session.environment_state = Some(environment_state);
        session.updated_at = Utc::now();
        save_session_record(&connection, &session)
    }

    async fn append_run(&self, mut run: RunRecord) -> SessionStoreResult<()> {
        run.updated_at = Utc::now();
        let connection = self.lock()?;
        let mut session = load_session_record(&connection, &run.session_id)?;
        save_run_record(&connection, &run)?;
        apply_run_to_session(&mut session, &run);
        save_session_record(&connection, &session)
    }

    async fn load_run(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<RunRecord> {
        let connection = self.lock()?;
        load_run_record(&connection, session_id, run_id)
    }

    async fn list_runs(&self, session_id: &SessionId) -> SessionStoreResult<Vec<RunRecord>> {
        let connection = self.lock()?;
        list_run_records(&connection, session_id)
    }

    async fn update_run_status(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        status: RunStatus,
        output_preview: Option<String>,
    ) -> SessionStoreResult<()> {
        let connection = self.lock()?;
        let mut run = load_run_record(&connection, session_id, run_id)?;
        run.status = status;
        run.output_preview = output_preview;
        run.updated_at = Utc::now();
        save_run_record(&connection, &run)?;
        let mut session = load_session_record(&connection, session_id)?;
        apply_run_to_session(&mut session, &run);
        save_session_record(&connection, &session)
    }

    async fn append_checkpoint(
        &self,
        session_id: &SessionId,
        checkpoint: AgentCheckpoint,
    ) -> SessionStoreResult<()> {
        let connection = self.lock()?;
        let key = run_key_label(session_id, &checkpoint.run_id);
        let mut run = load_run_record(&connection, session_id, &checkpoint.run_id)
            .map_err(|_| SessionStoreError::NotFound(key.clone()))?;
        let created_at = Utc::now();
        let payload = serialize(&checkpoint)?;
        connection
            .execute(
                "INSERT OR REPLACE INTO checkpoints
                 (session_id, run_id, sequence_no, checkpoint_id, record, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    session_id.as_str(),
                    checkpoint.run_id.as_str(),
                    i64::try_from(checkpoint.run_step).map_err(int_error)?,
                    checkpoint.checkpoint_id.as_str(),
                    payload,
                    created_at.to_rfc3339(),
                ],
            )
            .map_err(sql_error)?;
        run.latest_checkpoint = Some(starweaver_session::CheckpointRef {
            checkpoint_id: checkpoint.checkpoint_id,
            run_id: checkpoint.run_id,
            sequence: checkpoint.run_step,
            node: format!("{:?}", checkpoint.node),
            storage_ref: None,
            stream_cursor: checkpoint.resume.cursor.stream_cursor,
            created_at,
            metadata: checkpoint.metadata,
        });
        run.updated_at = created_at;
        save_run_record(&connection, &run)
    }

    async fn load_checkpoints(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<Vec<AgentCheckpoint>> {
        let connection = self.lock()?;
        let mut statement = connection
            .prepare(
                "SELECT record FROM checkpoints
                 WHERE session_id = ?1 AND run_id = ?2
                 ORDER BY sequence_no ASC, checkpoint_id ASC",
            )
            .map_err(sql_error)?;
        let rows = statement
            .query_map(params![session_id.as_str(), run_id.as_str()], |row| {
                row.get::<_, String>(0)
            })
            .map_err(sql_error)?;
        collect_json_rows(rows)
    }

    async fn append_stream_records(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        records: Vec<AgentStreamRecord>,
    ) -> SessionStoreResult<()> {
        let connection = self.lock()?;
        let mut run = load_run_record(&connection, session_id, run_id)?;
        for record in records {
            connection
                .execute(
                    "INSERT OR REPLACE INTO stream_records
                     (session_id, run_id, sequence_no, record)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![
                        session_id.as_str(),
                        run_id.as_str(),
                        i64::try_from(record.sequence).map_err(int_error)?,
                        serialize(&record)?,
                    ],
                )
                .map_err(sql_error)?;
        }
        let latest_sequence = latest_stream_sequence(&connection, session_id, run_id)?;
        if let Some(sequence) = latest_sequence {
            let cursor =
                StreamCursorRef::new("raw_runtime", format!("run:{}", run_id.as_str()), sequence);
            run.stream_cursors
                .retain(|existing| existing.family != cursor.family);
            run.stream_cursors.push(cursor.clone());
            run.updated_at = Utc::now();
            save_run_record(&connection, &run)?;
            let mut session = load_session_record(&connection, session_id)?;
            session.stream_cursors.retain(|existing| {
                existing.family != cursor.family || existing.scope != cursor.scope
            });
            session.stream_cursors.push(cursor);
            session.updated_at = run.updated_at;
            save_session_record(&connection, &session)?;
        }
        Ok(())
    }

    async fn replay_stream_records(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<Vec<AgentStreamRecord>> {
        let connection = self.lock()?;
        let mut statement = connection
            .prepare(
                "SELECT record FROM stream_records
                 WHERE session_id = ?1 AND run_id = ?2
                 ORDER BY sequence_no ASC",
            )
            .map_err(sql_error)?;
        let rows = statement
            .query_map(params![session_id.as_str(), run_id.as_str()], |row| {
                row.get::<_, String>(0)
            })
            .map_err(sql_error)?;
        collect_json_rows(rows)
    }

    async fn save_stream_cursor(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        cursor: StreamCursorRef,
    ) -> SessionStoreResult<()> {
        let connection = self.lock()?;
        let mut run = load_run_record(&connection, session_id, run_id)?;
        run.stream_cursors
            .retain(|existing| existing.family != cursor.family || existing.scope != cursor.scope);
        run.stream_cursors.push(cursor.clone());
        run.updated_at = Utc::now();
        save_run_record(&connection, &run)?;

        let mut session = load_session_record(&connection, session_id)?;
        session
            .stream_cursors
            .retain(|existing| existing.family != cursor.family || existing.scope != cursor.scope);
        session.stream_cursors.push(cursor);
        session.updated_at = run.updated_at;
        save_session_record(&connection, &session)
    }

    async fn append_approval(&self, approval: ApprovalRecord) -> SessionStoreResult<()> {
        let connection = self.lock()?;
        let _run = load_run_record(&connection, &approval.session_id, &approval.run_id)?;
        connection
            .execute(
                "INSERT OR REPLACE INTO approvals
                 (session_id, run_id, approval_id, record, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    approval.session_id.as_str(),
                    approval.run_id.as_str(),
                    approval.approval_id,
                    serialize(&approval)?,
                    Utc::now().to_rfc3339(),
                ],
            )
            .map_err(sql_error)?;
        Ok(())
    }

    async fn load_approvals(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<Vec<ApprovalRecord>> {
        let connection = self.lock()?;
        let mut statement = connection
            .prepare(
                "SELECT record FROM approvals
                 WHERE session_id = ?1 AND run_id = ?2
                 ORDER BY updated_at ASC, approval_id ASC",
            )
            .map_err(sql_error)?;
        let rows = statement
            .query_map(params![session_id.as_str(), run_id.as_str()], |row| {
                row.get::<_, String>(0)
            })
            .map_err(sql_error)?;
        collect_json_rows(rows)
    }

    async fn append_deferred_tool(&self, record: DeferredToolRecord) -> SessionStoreResult<()> {
        let connection = self.lock()?;
        let _run = load_run_record(&connection, &record.session_id, &record.run_id)?;
        connection
            .execute(
                "INSERT OR REPLACE INTO deferred_tools
                 (session_id, run_id, deferred_id, record, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    record.session_id.as_str(),
                    record.run_id.as_str(),
                    record.deferred_id,
                    serialize(&record)?,
                    Utc::now().to_rfc3339(),
                ],
            )
            .map_err(sql_error)?;
        Ok(())
    }

    async fn load_deferred_tools(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<Vec<DeferredToolRecord>> {
        let connection = self.lock()?;
        let mut statement = connection
            .prepare(
                "SELECT record FROM deferred_tools
                 WHERE session_id = ?1 AND run_id = ?2
                 ORDER BY updated_at ASC, deferred_id ASC",
            )
            .map_err(sql_error)?;
        let rows = statement
            .query_map(params![session_id.as_str(), run_id.as_str()], |row| {
                row.get::<_, String>(0)
            })
            .map_err(sql_error)?;
        collect_json_rows(rows)
    }

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

    async fn compact_run_trace(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<CompactRunTrace> {
        let connection = self.lock()?;
        let run = load_run_record(&connection, session_id, run_id)?;
        let checkpoints = load_checkpoint_ids(&connection, session_id, run_id)?;
        let stream_cursor = latest_stream_sequence(&connection, session_id, run_id)?;
        let approvals = count_pending_approvals(&connection, session_id, run_id)?;
        let deferred_tools = count_deferred_tools(&connection, session_id, run_id)?;
        Ok(CompactRunTrace {
            session_id: Some(session_id.clone()),
            run_id: Some(run_id.clone()),
            status: run.status,
            checkpoints: checkpoints.clone(),
            approvals,
            deferred_tools,
            latest_checkpoint: checkpoints.last().cloned(),
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
        let connection = self.lock()?;
        let session = load_session_record(&connection, session_id)?;
        let runs = list_run_records(&connection, session_id)?;
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

fn save_session_record(connection: &Connection, session: &SessionRecord) -> SessionStoreResult<()> {
    connection
        .execute(
            "INSERT OR REPLACE INTO sessions (session_id, record, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                session.session_id.as_str(),
                serialize(session)?,
                session.created_at.to_rfc3339(),
                session.updated_at.to_rfc3339(),
            ],
        )
        .map_err(sql_error)?;
    Ok(())
}

fn load_session_record(
    connection: &Connection,
    session_id: &SessionId,
) -> SessionStoreResult<SessionRecord> {
    let payload = connection
        .query_row(
            "SELECT record FROM sessions WHERE session_id = ?1",
            params![session_id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(sql_error)?
        .ok_or_else(|| SessionStoreError::NotFound(session_id.as_str().to_string()))?;
    deserialize(&payload)
}

fn save_run_record(connection: &Connection, run: &RunRecord) -> SessionStoreResult<()> {
    connection
        .execute(
            "INSERT OR REPLACE INTO runs
             (session_id, run_id, record, sequence_no, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                run.session_id.as_str(),
                run.run_id.as_str(),
                serialize(run)?,
                i64::try_from(run.sequence_no).map_err(int_error)?,
                run.created_at.to_rfc3339(),
                run.updated_at.to_rfc3339(),
            ],
        )
        .map_err(sql_error)?;
    Ok(())
}

fn load_run_record(
    connection: &Connection,
    session_id: &SessionId,
    run_id: &RunId,
) -> SessionStoreResult<RunRecord> {
    let payload = connection
        .query_row(
            "SELECT record FROM runs WHERE session_id = ?1 AND run_id = ?2",
            params![session_id.as_str(), run_id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(sql_error)?
        .ok_or_else(|| SessionStoreError::NotFound(run_key_label(session_id, run_id)))?;
    deserialize(&payload)
}

fn list_run_records(
    connection: &Connection,
    session_id: &SessionId,
) -> SessionStoreResult<Vec<RunRecord>> {
    let mut statement = connection
        .prepare("SELECT record FROM runs WHERE session_id = ?1 ORDER BY sequence_no ASC")
        .map_err(sql_error)?;
    let rows = statement
        .query_map(params![session_id.as_str()], |row| row.get::<_, String>(0))
        .map_err(sql_error)?;
    collect_json_rows(rows)
}

fn apply_run_to_session(session: &mut SessionRecord, run: &RunRecord) {
    session.head_run_id = Some(run.run_id.clone());
    match run.status {
        RunStatus::Queued | RunStatus::Running | RunStatus::Waiting => {
            session.active_run_id = Some(run.run_id.clone());
        }
        RunStatus::Completed => {
            session.head_success_run_id = Some(run.run_id.clone());
            if session.active_run_id.as_ref() == Some(&run.run_id) {
                session.active_run_id = None;
            }
        }
        RunStatus::Failed | RunStatus::Cancelled => {
            if session.active_run_id.as_ref() == Some(&run.run_id) {
                session.active_run_id = None;
            }
        }
    }
    session.updated_at = run.updated_at;
}

fn latest_stream_sequence(
    connection: &Connection,
    session_id: &SessionId,
    run_id: &RunId,
) -> SessionStoreResult<Option<usize>> {
    let value = connection
        .query_row(
            "SELECT MAX(sequence_no) FROM stream_records WHERE session_id = ?1 AND run_id = ?2",
            params![session_id.as_str(), run_id.as_str()],
            |row| row.get::<_, Option<i64>>(0),
        )
        .map_err(sql_error)?;
    value
        .map(|sequence| usize::try_from(sequence).map_err(int_error))
        .transpose()
}

fn load_checkpoint_ids(
    connection: &Connection,
    session_id: &SessionId,
    run_id: &RunId,
) -> SessionStoreResult<Vec<starweaver_core::CheckpointId>> {
    let mut statement = connection
        .prepare(
            "SELECT record FROM checkpoints
             WHERE session_id = ?1 AND run_id = ?2
             ORDER BY sequence_no ASC, checkpoint_id ASC",
        )
        .map_err(sql_error)?;
    let rows = statement
        .query_map(params![session_id.as_str(), run_id.as_str()], |row| {
            row.get::<_, String>(0)
        })
        .map_err(sql_error)?;
    let checkpoints = collect_json_rows::<AgentCheckpoint>(rows)?;
    Ok(checkpoints
        .into_iter()
        .map(|checkpoint| checkpoint.checkpoint_id)
        .collect())
}

fn count_pending_approvals(
    connection: &Connection,
    session_id: &SessionId,
    run_id: &RunId,
) -> SessionStoreResult<usize> {
    let approvals = {
        let mut statement = connection
            .prepare("SELECT record FROM approvals WHERE session_id = ?1 AND run_id = ?2")
            .map_err(sql_error)?;
        let rows = statement
            .query_map(params![session_id.as_str(), run_id.as_str()], |row| {
                row.get::<_, String>(0)
            })
            .map_err(sql_error)?;
        collect_json_rows::<ApprovalRecord>(rows)?
    };
    Ok(approvals
        .iter()
        .filter(|approval| approval.status == ApprovalStatus::Pending)
        .count())
}

fn count_deferred_tools(
    connection: &Connection,
    session_id: &SessionId,
    run_id: &RunId,
) -> SessionStoreResult<usize> {
    let count = connection
        .query_row(
            "SELECT COUNT(*) FROM deferred_tools WHERE session_id = ?1 AND run_id = ?2",
            params![session_id.as_str(), run_id.as_str()],
            |row| row.get::<_, i64>(0),
        )
        .map_err(sql_error)?;
    usize::try_from(count).map_err(int_error)
}

fn collect_json_rows<T>(
    rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<String>>,
) -> SessionStoreResult<Vec<T>>
where
    T: serde::de::DeserializeOwned,
{
    let mut values = Vec::new();
    for row in rows {
        values.push(deserialize(&row.map_err(sql_error)?)?);
    }
    Ok(values)
}

fn serialize<T>(value: &T) -> SessionStoreResult<String>
where
    T: serde::Serialize,
{
    serde_json::to_string(value).map_err(|error| SessionStoreError::Failed(error.to_string()))
}

fn deserialize<T>(payload: &str) -> SessionStoreResult<T>
where
    T: serde::de::DeserializeOwned,
{
    serde_json::from_str(payload).map_err(|error| SessionStoreError::Failed(error.to_string()))
}

fn sql_error(error: rusqlite::Error) -> SessionStoreError {
    SessionStoreError::Failed(error.to_string())
}

fn int_error(error: impl std::fmt::Display) -> SessionStoreError {
    SessionStoreError::Failed(error.to_string())
}

fn run_key_label(session_id: &SessionId, run_id: &RunId) -> String {
    format!("{}:{}", session_id.as_str(), run_id.as_str())
}

#[cfg(test)]
mod tests {
    use starweaver_core::ConversationId;

    use super::*;

    #[tokio::test]
    async fn sqlite_store_round_trips_session_and_run() {
        let store = SqliteSessionStore::in_memory().expect("sqlite store");
        let session_id = SessionId::from_string("session_test");
        store
            .save_session(SessionRecord::new(session_id.clone()))
            .await
            .expect("save session");
        let run_id = RunId::from_string("run_test");
        let mut run = RunRecord::new(session_id.clone(), run_id.clone(), ConversationId::new());
        run.sequence_no = 1;
        store.append_run(run).await.expect("append run");

        let session = store.load_session(&session_id).await.expect("load session");
        assert_eq!(session.active_run_id.as_ref(), Some(&run_id));
        let runs = store.list_runs(&session_id).await.expect("list runs");
        assert_eq!(runs.len(), 1);
    }
}

/// SQLite-backed replay event log with in-process live subscriptions.
#[derive(Clone, Debug)]
pub struct SqliteReplayEventLog {
    connection: Arc<Mutex<Connection>>,
    live: InMemoryReplayEventLog,
}

impl SqliteReplayEventLog {
    /// Open or create a SQLite replay event log.
    ///
    /// # Errors
    ///
    /// Returns a replay error when SQLite cannot open or initialize the database.
    pub fn open(path: impl AsRef<Path>) -> ReplayResult<Self> {
        let connection = Connection::open(path).map_err(replay_sql_error)?;
        let log = Self {
            connection: Arc::new(Mutex::new(connection)),
            live: InMemoryReplayEventLog::new(),
        };
        log.migrate()?;
        Ok(log)
    }

    /// Open an in-memory SQLite replay event log.
    ///
    /// # Errors
    ///
    /// Returns a replay error when SQLite cannot initialize the database.
    pub fn in_memory() -> ReplayResult<Self> {
        let connection = Connection::open_in_memory().map_err(replay_sql_error)?;
        let log = Self {
            connection: Arc::new(Mutex::new(connection)),
            live: InMemoryReplayEventLog::new(),
        };
        log.migrate()?;
        Ok(log)
    }

    fn migrate(&self) -> ReplayResult<()> {
        let connection = self.replay_lock()?;
        connection
            .execute_batch(
                r"
                CREATE TABLE IF NOT EXISTS replay_events (
                    scope TEXT NOT NULL,
                    sequence_no INTEGER NOT NULL,
                    record TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    PRIMARY KEY (scope, sequence_no)
                );
                CREATE INDEX IF NOT EXISTS ix_replay_events_scope_sequence
                    ON replay_events(scope, sequence_no);

                CREATE TABLE IF NOT EXISTS replay_snapshots (
                    scope TEXT PRIMARY KEY,
                    record TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );
                ",
            )
            .map_err(replay_sql_error)?;
        Ok(())
    }

    fn replay_lock(&self) -> ReplayResult<MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|error| ReplayError::Failed(error.to_string()))
    }
}

#[async_trait]
impl ReplayEventLog for SqliteReplayEventLog {
    async fn append(&self, scope: ReplayScope, mut event: ReplayEvent) -> ReplayResult<()> {
        event.scope = scope.clone();
        {
            let connection = self.replay_lock()?;
            connection
                .execute(
                    "INSERT OR IGNORE INTO replay_events (scope, sequence_no, record, created_at)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![
                        scope.as_str(),
                        i64::try_from(event.sequence)
                            .map_err(|error| ReplayError::Failed(error.to_string()))?,
                        serde_json::to_string(&event)
                            .map_err(|error| ReplayError::Failed(error.to_string()))?,
                        event.timestamp.to_rfc3339(),
                    ],
                )
                .map_err(replay_sql_error)?;
        }
        self.live.append(scope, event).await
    }

    async fn replay_after(
        &self,
        scope: &ReplayScope,
        cursor: Option<ReplayCursor>,
        limit: Option<usize>,
    ) -> ReplayResult<Vec<ReplayEvent>> {
        if let Some(cursor) = cursor.as_ref() {
            cursor.validate_scope(scope)?;
        }
        let after = cursor.map_or(0, |cursor| cursor.sequence.saturating_add(1));
        let limit = limit.unwrap_or(1000);
        let connection = self.replay_lock()?;
        let mut statement = connection
            .prepare(
                "SELECT record FROM replay_events
                 WHERE scope = ?1 AND sequence_no >= ?2
                 ORDER BY sequence_no ASC
                 LIMIT ?3",
            )
            .map_err(replay_sql_error)?;
        let rows = statement
            .query_map(
                params![
                    scope.as_str(),
                    i64::try_from(after).map_err(|error| ReplayError::Failed(error.to_string()))?,
                    i64::try_from(limit).map_err(|error| ReplayError::Failed(error.to_string()))?,
                ],
                |row| row.get::<_, String>(0),
            )
            .map_err(replay_sql_error)?;
        let mut events = Vec::new();
        for row in rows {
            let payload = row.map_err(replay_sql_error)?;
            events.push(
                serde_json::from_str::<ReplayEvent>(&payload)
                    .map_err(|error| ReplayError::Failed(error.to_string()))?,
            );
        }
        Ok(events)
    }

    async fn subscribe(
        &self,
        scope: ReplayScope,
        cursor: Option<ReplayCursor>,
    ) -> ReplayResult<ReplaySubscription> {
        self.live.subscribe(scope, cursor).await
    }

    async fn compact_snapshot(&self, scope: &ReplayScope) -> ReplayResult<ReplaySnapshot> {
        let snapshot_payload = {
            let connection = self.replay_lock()?;
            connection
                .query_row(
                    "SELECT record FROM replay_snapshots WHERE scope = ?1",
                    params![scope.as_str()],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(replay_sql_error)?
        };
        if let Some(payload) = snapshot_payload {
            return serde_json::from_str::<ReplaySnapshot>(&payload)
                .map_err(|error| ReplayError::Failed(error.to_string()));
        }
        let events = self.replay_after(scope, None, None).await?;
        let display_messages = events
            .iter()
            .filter_map(|event| match &event.event {
                ReplayEventKind::DisplayMessage(message) => Some((**message).clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        let cursor = events
            .last()
            .map(|event| ReplayCursor::new(scope.clone(), event.sequence));
        Ok(ReplaySnapshot {
            scope: Some(scope.clone()),
            revision: events.len(),
            cursor,
            display_messages,
            metadata: Metadata::default(),
        })
    }
}

impl SqliteReplayEventLog {
    /// Persist a compact snapshot.
    ///
    /// # Errors
    ///
    /// Returns a replay error when SQLite write or JSON encoding fails.
    pub fn save_snapshot(&self, scope: ReplayScope, snapshot: ReplaySnapshot) -> ReplayResult<()> {
        let connection = self.replay_lock()?;
        connection
            .execute(
                "INSERT OR REPLACE INTO replay_snapshots (scope, record, updated_at)
                 VALUES (?1, ?2, ?3)",
                params![
                    scope.as_str(),
                    serde_json::to_string(&snapshot)
                        .map_err(|error| ReplayError::Failed(error.to_string()))?,
                    Utc::now().to_rfc3339(),
                ],
            )
            .map_err(replay_sql_error)?;
        Ok(())
    }
}

fn replay_sql_error(error: rusqlite::Error) -> ReplayError {
    ReplayError::Failed(error.to_string())
}

#[cfg(test)]
mod replay_tests {
    use super::*;

    #[tokio::test]
    async fn sqlite_replay_log_round_trips_events() {
        let log = SqliteReplayEventLog::in_memory().expect("replay log");
        let scope = ReplayScope::run("run_test");
        log.append(
            scope.clone(),
            ReplayEvent::new(scope.clone(), 1, ReplayEventKind::Heartbeat),
        )
        .await
        .expect("append event");
        let events = log.replay_after(&scope, None, None).await.expect("replay");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].sequence, 1);
    }
}
