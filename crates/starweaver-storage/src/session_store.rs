//! SQLite-backed durable session store adapter.

use std::{
    path::Path,
    sync::{Arc, Mutex, MutexGuard},
};

use async_trait::async_trait;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use starweaver_context::ResumableState;
use starweaver_core::{RunId, SessionId};
use starweaver_runtime::{AgentCheckpoint, AgentStreamRecord};
use starweaver_session::{
    ApprovalRecord, ApprovalStatus, CompactRunTrace, CompactSessionTrace, DeferredToolRecord,
    EnvironmentStateRef, RunRecord, RunStatus, SessionFilter, SessionRecord, SessionResumeSnapshot,
    SessionStatus, SessionStore, SessionStoreError, SessionStoreResult, StreamCursorRef,
};

use crate::{
    connection::{open_connection, open_in_memory_connection},
    errors::{collect_json_rows, deserialize, int_error, run_key_label, serialize, sql_error},
    migrations::apply_sqlite_migrations,
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
        let mut connection = open_connection(path)?;
        apply_sqlite_migrations(&mut connection)?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
        })
    }

    /// Open an in-memory SQLite session store.
    ///
    /// # Errors
    ///
    /// Returns a store error when SQLite cannot initialize the database.
    pub fn in_memory() -> SessionStoreResult<Self> {
        let mut connection = open_in_memory_connection()?;
        apply_sqlite_migrations(&mut connection)?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
        })
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
            .prepare("SELECT record FROM session_records ORDER BY updated_at DESC")
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
            "INSERT OR REPLACE INTO session_records (session_id, record, created_at, updated_at)
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
            "SELECT record FROM session_records WHERE session_id = ?1",
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
            "INSERT OR REPLACE INTO run_records
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
            "SELECT record FROM run_records WHERE session_id = ?1 AND run_id = ?2",
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
        .prepare("SELECT record FROM run_records WHERE session_id = ?1 ORDER BY sequence_no ASC")
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
