//! SQLite-backed stream archive adapter.

use std::{
    path::Path,
    sync::{Arc, Mutex, MutexGuard},
};

use async_trait::async_trait;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use starweaver_core::{RunId, SessionId};
use starweaver_runtime::AgentStreamRecord;
use starweaver_stream::{
    DisplayMessage, ReplayCursor, ReplayError, ReplayEvent, ReplayEventKind, ReplayResult,
    ReplayScope, ReplaySnapshot, StreamArchive,
};

use crate::{
    migrations::apply_sqlite_migrations,
    sqlite::{
        map_session_to_replay_error, map_sqlite_replay_error, open_in_memory_sqlite_connection,
        open_sqlite_connection,
    },
};

/// SQLite-backed stream archive for raw runtime records, display replay, and snapshots.
#[derive(Clone, Debug)]
pub struct SqliteStreamArchive {
    connection: Arc<Mutex<Connection>>,
}

impl SqliteStreamArchive {
    /// Open or create a SQLite stream archive.
    ///
    /// # Errors
    ///
    /// Returns a replay error when SQLite cannot open or initialize the database.
    pub fn open(path: impl AsRef<Path>) -> ReplayResult<Self> {
        let mut connection = open_sqlite_connection(path).map_err(map_session_to_replay_error)?;
        apply_sqlite_migrations(&mut connection).map_err(map_session_to_replay_error)?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
        })
    }

    /// Open an in-memory SQLite stream archive.
    ///
    /// # Errors
    ///
    /// Returns a replay error when SQLite cannot initialize the database.
    pub fn in_memory() -> ReplayResult<Self> {
        let mut connection =
            open_in_memory_sqlite_connection().map_err(map_session_to_replay_error)?;
        apply_sqlite_migrations(&mut connection).map_err(map_session_to_replay_error)?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
        })
    }

    fn archive_lock(&self) -> ReplayResult<MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|error| ReplayError::Failed(error.to_string()))
    }
}

#[async_trait]
impl StreamArchive for SqliteStreamArchive {
    async fn append_raw_records(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        records: Vec<AgentStreamRecord>,
    ) -> ReplayResult<()> {
        let connection = self.archive_lock()?;
        for record in records {
            connection
                .execute(
                    "INSERT OR REPLACE INTO stream_records
                     (session_id, run_id, sequence_no, record)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![
                        session_id.as_str(),
                        run_id.as_str(),
                        i64::try_from(record.sequence)
                            .map_err(|error| ReplayError::Failed(error.to_string()))?,
                        serde_json::to_string(&record)
                            .map_err(|error| ReplayError::Failed(error.to_string()))?,
                    ],
                )
                .map_err(map_sqlite_replay_error)?;
        }
        Ok(())
    }

    async fn replay_raw_after(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        cursor: Option<ReplayCursor>,
    ) -> ReplayResult<Vec<AgentStreamRecord>> {
        if let Some(cursor) = cursor.as_ref() {
            cursor.validate_scope(&ReplayScope::run(run_id.as_str()))?;
        }
        let after = cursor.map_or(0, |cursor| cursor.sequence.saturating_add(1));
        let connection = self.archive_lock()?;
        let mut statement = connection
            .prepare(
                "SELECT record FROM stream_records
                 WHERE session_id = ?1 AND run_id = ?2 AND sequence_no >= ?3
                 ORDER BY sequence_no ASC",
            )
            .map_err(map_sqlite_replay_error)?;
        let rows = statement
            .query_map(
                params![
                    session_id.as_str(),
                    run_id.as_str(),
                    i64::try_from(after).map_err(|error| ReplayError::Failed(error.to_string()))?,
                ],
                |row| row.get::<_, String>(0),
            )
            .map_err(map_sqlite_replay_error)?;
        let mut records = Vec::new();
        for row in rows {
            records.push(
                serde_json::from_str::<AgentStreamRecord>(&row.map_err(map_sqlite_replay_error)?)
                    .map_err(|error| ReplayError::Failed(error.to_string()))?,
            );
        }
        Ok(records)
    }

    async fn append_display_messages(
        &self,
        scope: ReplayScope,
        messages: Vec<DisplayMessage>,
    ) -> ReplayResult<()> {
        let connection = self.archive_lock()?;
        for message in messages {
            let event = ReplayEvent::display(scope.clone(), message);
            connection
                .execute(
                    "INSERT OR REPLACE INTO replay_events (scope, sequence_no, record, created_at)
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
                .map_err(map_sqlite_replay_error)?;
        }
        Ok(())
    }

    async fn replay_display_after(
        &self,
        scope: &ReplayScope,
        cursor: Option<ReplayCursor>,
    ) -> ReplayResult<Vec<DisplayMessage>> {
        if let Some(cursor) = cursor.as_ref() {
            cursor.validate_scope(scope)?;
        }
        let after = cursor.map_or(0, |cursor| cursor.sequence.saturating_add(1));
        let connection = self.archive_lock()?;
        let mut statement = connection
            .prepare(
                "SELECT record FROM replay_events
                 WHERE scope = ?1 AND sequence_no >= ?2
                 ORDER BY sequence_no ASC",
            )
            .map_err(map_sqlite_replay_error)?;
        let rows = statement
            .query_map(
                params![
                    scope.as_str(),
                    i64::try_from(after).map_err(|error| ReplayError::Failed(error.to_string()))?,
                ],
                |row| row.get::<_, String>(0),
            )
            .map_err(map_sqlite_replay_error)?;
        let mut messages = Vec::new();
        for row in rows {
            let event = serde_json::from_str::<ReplayEvent>(&row.map_err(map_sqlite_replay_error)?)
                .map_err(|error| ReplayError::Failed(error.to_string()))?;
            if let ReplayEventKind::DisplayMessage(message) = event.event {
                messages.push(*message);
            }
        }
        Ok(messages)
    }

    async fn append_snapshot(
        &self,
        scope: ReplayScope,
        snapshot: ReplaySnapshot,
    ) -> ReplayResult<()> {
        let connection = self.archive_lock()?;
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
            .map_err(map_sqlite_replay_error)?;
        Ok(())
    }

    async fn latest_snapshot(&self, scope: &ReplayScope) -> ReplayResult<Option<ReplaySnapshot>> {
        let connection = self.archive_lock()?;
        let payload = connection
            .query_row(
                "SELECT record FROM replay_snapshots WHERE scope = ?1",
                params![scope.as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(map_sqlite_replay_error)?;
        payload
            .map(|payload| {
                serde_json::from_str::<ReplaySnapshot>(&payload)
                    .map_err(|error| ReplayError::Failed(error.to_string()))
            })
            .transpose()
    }

    async fn cursor_range(
        &self,
        scope: &ReplayScope,
    ) -> ReplayResult<Option<(ReplayCursor, ReplayCursor)>> {
        let connection = self.archive_lock()?;
        let range = connection
            .query_row(
                "SELECT MIN(sequence_no), MAX(sequence_no) FROM replay_events WHERE scope = ?1",
                params![scope.as_str()],
                |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, Option<i64>>(1)?)),
            )
            .map_err(map_sqlite_replay_error)?;
        let (Some(first), Some(last)) = range else {
            return Ok(None);
        };
        Ok(Some((
            ReplayCursor::new(
                scope.clone(),
                usize::try_from(first).map_err(|error| ReplayError::Failed(error.to_string()))?,
            ),
            ReplayCursor::new(
                scope.clone(),
                usize::try_from(last).map_err(|error| ReplayError::Failed(error.to_string()))?,
            ),
        )))
    }
}
