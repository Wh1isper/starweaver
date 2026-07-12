//! SQLite-backed replay event log adapter.

use std::{
    path::Path,
    sync::{Arc, Mutex, MutexGuard},
};

use async_trait::async_trait;
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, params};
use starweaver_core::{Metadata, from_versioned_json, to_versioned_json};
use starweaver_stream::{
    InMemoryReplayEventLog, ReplayCatchupSource, ReplayCursor, ReplayCursorFamily, ReplayError,
    ReplayEvent, ReplayEventKind, ReplayEventLog, ReplayResult, ReplayScope, ReplaySnapshot,
    ReplaySubscription,
};

use crate::{
    SqliteStorage,
    sqlite::{SharedSqliteConnection, map_sqlite_replay_error},
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
        SqliteStorage::open(path)
            .map(|storage| storage.replay_event_log())
            .map_err(crate::sqlite::map_session_to_replay_error)
    }

    /// Open an in-memory SQLite replay event log.
    ///
    /// # Errors
    ///
    /// Returns a replay error when SQLite cannot initialize the database.
    pub fn in_memory() -> ReplayResult<Self> {
        SqliteStorage::in_memory()
            .map(|storage| storage.replay_event_log())
            .map_err(crate::sqlite::map_session_to_replay_error)
    }

    pub(crate) fn from_shared(
        connection: SharedSqliteConnection,
        live: InMemoryReplayEventLog,
    ) -> Self {
        Self { connection, live }
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
        let sequence = i64::try_from(event.sequence)
            .map_err(|error| ReplayError::Failed(error.to_string()))?;
        let payload =
            to_versioned_json(&event).map_err(|error| ReplayError::Failed(error.to_string()))?;
        let store = self.clone();
        let durable_scope = scope.clone();
        let durable_event = event.clone();
        let _inserted = crate::blocking::run(move || -> ReplayResult<bool> {
            let connection = store.replay_lock()?;
            let inserted = connection
                .execute(
                    "INSERT OR IGNORE INTO replay_events (scope, sequence_no, record, created_at)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![
                        durable_scope.as_str(),
                        sequence,
                        payload,
                        durable_event.timestamp.to_rfc3339(),
                    ],
                )
                .map_err(map_sqlite_replay_error)?;
            if inserted == 0 {
                let persisted = connection
                    .query_row(
                        "SELECT record FROM replay_events WHERE scope = ?1 AND sequence_no = ?2",
                        params![durable_scope.as_str(), sequence],
                        |row| row.get::<_, String>(0),
                    )
                    .map_err(map_sqlite_replay_error)?;
                let persisted = from_versioned_json::<ReplayEvent>(&persisted)
                    .map_err(|error| ReplayError::Failed(error.to_string()))?;
                if persisted != durable_event {
                    return Err(ReplayError::Failed(format!(
                        "replay event conflict for scope {} at sequence {}",
                        durable_scope.as_str(),
                        durable_event.sequence
                    )));
                }
            }
            Ok(inserted == 1)
        })
        .await
        .map_err(ReplayError::Failed)??;
        // Always reconcile the durable event into the in-process live log. If a previous call
        // committed SQLite but failed before live publication, an identical retry must repair the
        // live side instead of treating the durable duplicate as fully delivered. The in-memory
        // log is itself idempotent, so already-published events are not emitted twice.
        self.live.append(scope, event).await?;
        Ok(())
    }

    async fn replay_after(
        &self,
        scope: &ReplayScope,
        cursor: Option<ReplayCursor>,
        limit: Option<usize>,
    ) -> ReplayResult<Vec<ReplayEvent>> {
        let store = self.clone();
        let scope = scope.clone();
        crate::blocking::run(move || {
            let scope = &scope;
            if let Some(cursor) = cursor.as_ref() {
                cursor.validate(ReplayCursorFamily::ReplayEvent, scope)?;
            }
            let after = cursor.map_or(0, |cursor| cursor.sequence.saturating_add(1));
            let connection = store.replay_lock()?;
            let after =
                i64::try_from(after).map_err(|error| ReplayError::Failed(error.to_string()))?;
            let mut events = Vec::new();
            if let Some(limit) = limit {
                let mut statement = connection
                    .prepare(
                        "SELECT record FROM replay_events
                     WHERE scope = ?1 AND sequence_no >= ?2
                     ORDER BY sequence_no ASC
                     LIMIT ?3",
                    )
                    .map_err(map_sqlite_replay_error)?;
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
                    .map_err(map_sqlite_replay_error)?;
                for row in rows {
                    let payload = row.map_err(map_sqlite_replay_error)?;
                    events.push(
                        from_versioned_json::<ReplayEvent>(&payload)
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
                    .map_err(map_sqlite_replay_error)?;
                let rows = statement
                    .query_map(params![scope.as_str(), after], |row| {
                        row.get::<_, String>(0)
                    })
                    .map_err(map_sqlite_replay_error)?;
                for row in rows {
                    let payload = row.map_err(map_sqlite_replay_error)?;
                    events.push(
                        from_versioned_json::<ReplayEvent>(&payload)
                            .map_err(|error| ReplayError::Failed(error.to_string()))?,
                    );
                }
            }
            Ok(events)
        })
        .await
        .map_err(ReplayError::Failed)?
    }

    async fn subscribe(
        &self,
        scope: ReplayScope,
        cursor: Option<ReplayCursor>,
    ) -> ReplayResult<ReplaySubscription> {
        // Establish the live receiver first, then capture the durable backlog. Events committed
        // between these operations can appear in both sources, but the subscription cursor
        // suppresses the duplicate after draining the ordered backlog.
        let mut subscription = self.live.subscribe(scope.clone(), cursor.clone()).await?;
        subscription.set_catchup_source(Arc::new(self.clone()));
        let backlog = <Self as ReplayEventLog>::replay_after(self, &scope, cursor, None).await?;
        subscription.initialize_backlog(backlog);
        Ok(subscription)
    }

    async fn compact_snapshot(&self, scope: &ReplayScope) -> ReplayResult<ReplaySnapshot> {
        let store = self.clone();
        let durable_scope = scope.clone();
        let snapshot_payload = crate::blocking::run(move || {
            let connection = store.replay_lock()?;
            connection
                .query_row(
                    "SELECT record FROM replay_snapshot_records WHERE scope = ?1",
                    params![durable_scope.as_str()],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(map_sqlite_replay_error)
        })
        .await
        .map_err(ReplayError::Failed)??;
        if let Some(payload) = snapshot_payload {
            let snapshot = from_versioned_json::<ReplaySnapshot>(&payload)
                .map_err(|error| ReplayError::Failed(error.to_string()))?;
            snapshot.validate(ReplayCursorFamily::ReplayEvent, scope)?;
            return Ok(snapshot);
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
            .map(|event| ReplayCursor::replay_event(scope.clone(), event.sequence));
        Ok(ReplaySnapshot {
            scope: Some(scope.clone()),
            revision: events.len(),
            cursor,
            display_messages,
            metadata: Metadata::default(),
        })
    }
}

#[async_trait]
impl ReplayCatchupSource for SqliteReplayEventLog {
    async fn catch_up_after(
        &self,
        scope: &ReplayScope,
        cursor: Option<ReplayCursor>,
        limit: Option<usize>,
    ) -> ReplayResult<Vec<ReplayEvent>> {
        <Self as ReplayEventLog>::replay_after(self, scope, cursor, limit).await
    }
}

impl SqliteReplayEventLog {
    /// Persist a compact snapshot.
    ///
    /// # Errors
    ///
    /// Returns a replay error when SQLite write or JSON encoding fails.
    pub fn save_snapshot(&self, scope: ReplayScope, snapshot: ReplaySnapshot) -> ReplayResult<()> {
        snapshot.validate(ReplayCursorFamily::ReplayEvent, &scope)?;
        let payload =
            to_versioned_json(&snapshot).map_err(|error| ReplayError::Failed(error.to_string()))?;
        {
            let connection = self.replay_lock()?;
            connection
                .execute(
                    "INSERT INTO replay_snapshot_records (scope, record, updated_at)
                     VALUES (?1, ?2, ?3)
                     ON CONFLICT(scope) DO UPDATE SET
                       record = excluded.record,
                       updated_at = excluded.updated_at",
                    params![scope.as_str(), payload, Utc::now().to_rfc3339()],
                )
                .map_err(map_sqlite_replay_error)?;
        }
        self.live.save_snapshot(scope, snapshot)
    }
}
