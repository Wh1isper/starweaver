//! SQLite-backed replay event log adapter.

use std::{
    path::Path,
    sync::{Arc, Mutex, MutexGuard},
};

use async_trait::async_trait;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use starweaver_core::Metadata;
use starweaver_stream::{
    InMemoryReplayEventLog, ReplayCursor, ReplayError, ReplayEvent, ReplayEventKind,
    ReplayEventLog, ReplayResult, ReplayScope, ReplaySnapshot, ReplaySubscription,
};

use crate::{
    connection::{open_connection, open_in_memory_connection},
    errors::{replay_sql_error, session_to_replay_error},
    migrations::apply_sqlite_migrations,
};

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
        let mut connection = open_connection(path).map_err(session_to_replay_error)?;
        apply_sqlite_migrations(&mut connection).map_err(session_to_replay_error)?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
            live: InMemoryReplayEventLog::new(),
        })
    }

    /// Open an in-memory SQLite replay event log.
    ///
    /// # Errors
    ///
    /// Returns a replay error when SQLite cannot initialize the database.
    pub fn in_memory() -> ReplayResult<Self> {
        let mut connection = open_in_memory_connection().map_err(session_to_replay_error)?;
        apply_sqlite_migrations(&mut connection).map_err(session_to_replay_error)?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
            live: InMemoryReplayEventLog::new(),
        })
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
        let connection = self.replay_lock()?;
        let after = i64::try_from(after).map_err(|error| ReplayError::Failed(error.to_string()))?;
        let mut events = Vec::new();
        if let Some(limit) = limit {
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
                        after,
                        i64::try_from(limit)
                            .map_err(|error| ReplayError::Failed(error.to_string()))?,
                    ],
                    |row| row.get::<_, String>(0),
                )
                .map_err(replay_sql_error)?;
            for row in rows {
                let payload = row.map_err(replay_sql_error)?;
                events.push(
                    serde_json::from_str::<ReplayEvent>(&payload)
                        .map_err(|error| ReplayError::Failed(error.to_string()))?,
                );
            }
        } else {
            let mut statement = connection
                .prepare(
                    "SELECT record FROM replay_events
                     WHERE scope = ?1 AND sequence_no >= ?2
                     ORDER BY sequence_no ASC",
                )
                .map_err(replay_sql_error)?;
            let rows = statement
                .query_map(params![scope.as_str(), after], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(replay_sql_error)?;
            for row in rows {
                let payload = row.map_err(replay_sql_error)?;
                events.push(
                    serde_json::from_str::<ReplayEvent>(&payload)
                        .map_err(|error| ReplayError::Failed(error.to_string()))?,
                );
            }
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
