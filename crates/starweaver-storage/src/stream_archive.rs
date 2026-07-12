//! SQLite-backed stream archive adapter.

use std::{
    path::Path,
    sync::{Arc, Mutex, MutexGuard},
};

use async_trait::async_trait;
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use starweaver_core::{RunId, SessionId, from_versioned_json, to_versioned_json};
use starweaver_stream::{
    AgentStreamRecord, DisplayMessage, ReplayCursor, ReplayCursorFamily, ReplayError, ReplayEvent,
    ReplayEventKind, ReplayResult, ReplayScope, ReplaySnapshot, StreamArchive,
};

use crate::{
    SqliteStorage,
    sqlite::{SharedSqliteConnection, map_sqlite_replay_error},
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
        SqliteStorage::open(path)
            .map(|storage| storage.stream_archive())
            .map_err(crate::sqlite::map_session_to_replay_error)
    }

    /// Open an in-memory SQLite stream archive.
    ///
    /// # Errors
    ///
    /// Returns a replay error when SQLite cannot initialize the database.
    pub fn in_memory() -> ReplayResult<Self> {
        SqliteStorage::in_memory()
            .map(|storage| storage.stream_archive())
            .map_err(crate::sqlite::map_session_to_replay_error)
    }

    pub(crate) fn from_shared(connection: SharedSqliteConnection) -> Self {
        Self { connection }
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
        let store = self.clone();
        let session_id = session_id.clone();
        let run_id = run_id.clone();
        crate::blocking::run(move || {
            let session_id = &session_id;
            let run_id = &run_id;
            let records = records
                .into_iter()
                .map(|record| {
                    Ok((
                        i64::try_from(record.sequence)
                            .map_err(|error| ReplayError::Failed(error.to_string()))?,
                        to_versioned_json(&record)
                            .map_err(|error| ReplayError::Failed(error.to_string()))?,
                        record,
                    ))
                })
                .collect::<ReplayResult<Vec<_>>>()?;
            let mut connection = store.archive_lock()?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite_replay_error)?;
            for (sequence, payload, record) in records {
                let inserted = transaction
                    .execute(
                        "INSERT OR IGNORE INTO stream_records
                     (session_id, run_id, sequence_no, record)
                     VALUES (?1, ?2, ?3, ?4)",
                        params![session_id.as_str(), run_id.as_str(), sequence, payload],
                    )
                    .map_err(map_sqlite_replay_error)?;
                if inserted == 0 {
                    let persisted = transaction
                        .query_row(
                            "SELECT record FROM stream_records
                         WHERE session_id = ?1 AND run_id = ?2 AND sequence_no = ?3",
                            params![session_id.as_str(), run_id.as_str(), sequence],
                            |row| row.get::<_, String>(0),
                        )
                        .map_err(map_sqlite_replay_error)?;
                    let persisted = from_versioned_json::<AgentStreamRecord>(&persisted)
                        .map_err(|error| ReplayError::Failed(error.to_string()))?;
                    if persisted != record {
                        return Err(ReplayError::Failed(format!(
                            "raw stream record conflict for session {} run {} at sequence {sequence}",
                            session_id.as_str(),
                            run_id.as_str()
                        )));
                    }
                }
            }
            transaction.commit().map_err(map_sqlite_replay_error)
        })
        .await
        .map_err(ReplayError::Failed)?
    }

    async fn replay_raw_after(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        cursor: Option<ReplayCursor>,
    ) -> ReplayResult<Vec<AgentStreamRecord>> {
        let store = self.clone();
        let session_id = session_id.clone();
        let run_id = run_id.clone();
        crate::blocking::run(move || {
            let session_id = &session_id;
            let run_id = &run_id;
            if let Some(cursor) = cursor.as_ref() {
                cursor.validate(
                    ReplayCursorFamily::RawRuntime,
                    &ReplayScope::run(run_id.as_str()),
                )?;
            }
            let after = cursor.map_or(0, |cursor| cursor.sequence.saturating_add(1));
            let connection = store.archive_lock()?;
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
                        i64::try_from(after)
                            .map_err(|error| ReplayError::Failed(error.to_string()))?,
                    ],
                    |row| row.get::<_, String>(0),
                )
                .map_err(map_sqlite_replay_error)?;
            let mut records = Vec::new();
            for row in rows {
                records.push(
                    from_versioned_json::<AgentStreamRecord>(
                        &row.map_err(map_sqlite_replay_error)?,
                    )
                    .map_err(|error| ReplayError::Failed(error.to_string()))?,
                );
            }
            Ok(records)
        })
        .await
        .map_err(ReplayError::Failed)?
    }

    async fn append_display_messages(
        &self,
        scope: ReplayScope,
        messages: Vec<DisplayMessage>,
    ) -> ReplayResult<()> {
        let store = self.clone();
        crate::blocking::run(move || {
            if messages.is_empty() {
                return Ok(());
            }
            let events = messages
                .into_iter()
                .map(|message| {
                    let event = ReplayEvent::display(scope.clone(), message);
                    Ok((
                        i64::try_from(event.sequence)
                            .map_err(|error| ReplayError::Failed(error.to_string()))?,
                        to_versioned_json(&event)
                            .map_err(|error| ReplayError::Failed(error.to_string()))?,
                        event.timestamp.to_rfc3339(),
                        event,
                    ))
                })
                .collect::<ReplayResult<Vec<_>>>()?;
            {
                let mut connection = store.archive_lock()?;
                let transaction = connection
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(map_sqlite_replay_error)?;
                for (sequence, payload, created_at, event) in events {
                    let inserted = transaction
                    .execute(
                        "INSERT OR IGNORE INTO display_message_records (scope, sequence_no, record, created_at)
                         VALUES (?1, ?2, ?3, ?4)",
                        params![scope.as_str(), sequence, payload, created_at],
                    )
                    .map_err(map_sqlite_replay_error)?;
                    if inserted == 0 {
                        let persisted = transaction
                        .query_row(
                            "SELECT record FROM display_message_records WHERE scope = ?1 AND sequence_no = ?2",
                            params![scope.as_str(), sequence],
                            |row| row.get::<_, String>(0),
                        )
                        .map_err(map_sqlite_replay_error)?;
                        let persisted = from_versioned_json::<ReplayEvent>(&persisted)
                            .map_err(|error| ReplayError::Failed(error.to_string()))?;
                        if persisted != event {
                            return Err(ReplayError::Failed(format!(
                                "replay event conflict for scope {} at sequence {sequence}",
                                scope.as_str()
                            )));
                        }
                    }
                }
                transaction.commit().map_err(map_sqlite_replay_error)?;
            }
            Ok(())
        })
        .await
        .map_err(ReplayError::Failed)?
    }

    async fn replay_display_after(
        &self,
        scope: &ReplayScope,
        cursor: Option<ReplayCursor>,
    ) -> ReplayResult<Vec<DisplayMessage>> {
        let store = self.clone();
        let scope = scope.clone();
        crate::blocking::run(move || {
            let scope = &scope;
            if let Some(cursor) = cursor.as_ref() {
                cursor.validate(ReplayCursorFamily::Display, scope)?;
            }
            let after = cursor.map_or(0, |cursor| cursor.sequence.saturating_add(1));
            let connection = store.archive_lock()?;
            let mut statement = connection
                .prepare(
                    "SELECT record FROM display_message_records
                 WHERE scope = ?1 AND sequence_no >= ?2
                 ORDER BY sequence_no ASC",
                )
                .map_err(map_sqlite_replay_error)?;
            let rows = statement
                .query_map(
                    params![
                        scope.as_str(),
                        i64::try_from(after)
                            .map_err(|error| ReplayError::Failed(error.to_string()))?,
                    ],
                    |row| row.get::<_, String>(0),
                )
                .map_err(map_sqlite_replay_error)?;
            let mut messages = Vec::new();
            for row in rows {
                let event =
                    from_versioned_json::<ReplayEvent>(&row.map_err(map_sqlite_replay_error)?)
                        .map_err(|error| ReplayError::Failed(error.to_string()))?;
                if let ReplayEventKind::DisplayMessage(message) = event.event {
                    messages.push(*message);
                }
            }
            Ok(messages)
        })
        .await
        .map_err(ReplayError::Failed)?
    }

    async fn append_snapshot(
        &self,
        scope: ReplayScope,
        snapshot: ReplaySnapshot,
    ) -> ReplayResult<()> {
        let store = self.clone();
        crate::blocking::run(move || {
            snapshot.validate(ReplayCursorFamily::Display, &scope)?;
            let payload = to_versioned_json(&snapshot)
                .map_err(|error| ReplayError::Failed(error.to_string()))?;
            {
                let connection = store.archive_lock()?;
                connection
                    .execute(
                        "INSERT INTO display_snapshot_records (scope, record, updated_at)
                     VALUES (?1, ?2, ?3)
                     ON CONFLICT(scope) DO UPDATE SET
                       record = excluded.record,
                       updated_at = excluded.updated_at",
                        params![scope.as_str(), payload, Utc::now().to_rfc3339()],
                    )
                    .map_err(map_sqlite_replay_error)?;
            }
            Ok(())
        })
        .await
        .map_err(ReplayError::Failed)?
    }

    async fn latest_snapshot(&self, scope: &ReplayScope) -> ReplayResult<Option<ReplaySnapshot>> {
        let store = self.clone();
        let scope = scope.clone();
        crate::blocking::run(move || {
            let scope = &scope;
            let connection = store.archive_lock()?;
            let payload = connection
                .query_row(
                    "SELECT record FROM display_snapshot_records WHERE scope = ?1",
                    params![scope.as_str()],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(map_sqlite_replay_error)?;
            payload
                .map(|payload| {
                    let snapshot = from_versioned_json::<ReplaySnapshot>(&payload)
                        .map_err(|error| ReplayError::Failed(error.to_string()))?;
                    snapshot.validate(ReplayCursorFamily::Display, scope)?;
                    Ok(snapshot)
                })
                .transpose()
        })
        .await
        .map_err(ReplayError::Failed)?
    }

    async fn cursor_range(
        &self,
        scope: &ReplayScope,
    ) -> ReplayResult<Option<(ReplayCursor, ReplayCursor)>> {
        let store = self.clone();
        let scope = scope.clone();
        crate::blocking::run(move || {
            let scope = &scope;
            let connection = store.archive_lock()?;
            let range = connection
            .query_row(
                "SELECT MIN(sequence_no), MAX(sequence_no) FROM display_message_records WHERE scope = ?1",
                params![scope.as_str()],
                |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, Option<i64>>(1)?)),
            )
            .map_err(map_sqlite_replay_error)?;
            let (Some(first), Some(last)) = range else {
                return Ok(None);
            };
            Ok(Some((
                ReplayCursor::display(
                    scope.clone(),
                    usize::try_from(first)
                        .map_err(|error| ReplayError::Failed(error.to_string()))?,
                ),
                ReplayCursor::display(
                    scope.clone(),
                    usize::try_from(last)
                        .map_err(|error| ReplayError::Failed(error.to_string()))?,
                ),
            )))
        })
        .await
        .map_err(ReplayError::Failed)?
    }
}
