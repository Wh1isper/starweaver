//! Unified SQLite storage resource and adapter construction.

use std::{
    path::Path,
    sync::{Arc, MutexGuard},
    time::Duration,
};

use chrono::{DateTime, Utc};
use rusqlite::{Connection, TransactionBehavior, params};
use starweaver_session::{
    AcquireRunAdmission, RunAdmissionLease, RunAdmissionReceipt, SessionRecord, SessionStoreError,
    SessionStoreResult,
};
use starweaver_stream::{InMemoryReplayEventLog, ReplayScope};

use crate::{
    migrations::apply_sqlite_migrations,
    replay_log::SqliteReplayEventLog,
    session_store::SqliteSessionStore,
    sqlite::{SharedSqliteConnection, open_in_memory_sqlite_connection, open_sqlite_connection},
    stream_archive::SqliteStreamArchive,
};

/// Durable source family selected for host replay of one scope.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DurableReplaySource {
    /// Typed canonical replay events.
    ReplayEvents,
    /// Durable user-visible display messages projected as replay events.
    DisplayMessages,
}

impl DurableReplaySource {
    const fn as_str(self) -> &'static str {
        match self {
            Self::ReplayEvents => "replay_events",
            Self::DisplayMessages => "display_messages",
        }
    }

    fn from_str(value: &str) -> SessionStoreResult<Self> {
        match value {
            "replay_events" => Ok(Self::ReplayEvents),
            "display_messages" => Ok(Self::DisplayMessages),
            other => Err(SessionStoreError::Failed(format!(
                "invalid durable replay source {other}"
            ))),
        }
    }
}

/// Unified SQLite storage resource for session, stream, and replay adapters.
///
/// Clones and derived adapters share one connection. Replay-event adapters additionally share an
/// in-process event bus; display archives remain a separate retained stream family. Separate
/// processes still use independent buses, while SQLite remains their durable source of truth.
#[derive(Clone, Debug)]
pub struct SqliteStorage {
    pub(crate) connection: SharedSqliteConnection,
    pub(crate) live_replay: InMemoryReplayEventLog,
}

impl SqliteStorage {
    /// Open or create a unified SQLite storage resource.
    ///
    /// # Errors
    ///
    /// Returns a store error when SQLite cannot open, configure, or migrate the database.
    pub fn open(path: impl AsRef<Path>) -> SessionStoreResult<Self> {
        let connection = open_sqlite_connection(path)?;
        Self::from_connection(connection)
    }

    /// Open a unified in-memory SQLite storage resource.
    ///
    /// # Errors
    ///
    /// Returns a store error when SQLite cannot configure or migrate the database.
    pub fn in_memory() -> SessionStoreResult<Self> {
        let connection = open_in_memory_sqlite_connection()?;
        Self::from_connection(connection)
    }

    fn from_connection(mut connection: Connection) -> SessionStoreResult<Self> {
        connection
            .busy_timeout(Duration::from_secs(10))
            .map_err(crate::sqlite::map_sqlite_session_error)?;
        apply_sqlite_migrations(&mut connection)?;
        Ok(Self {
            connection: Arc::new(std::sync::Mutex::new(connection)),
            live_replay: InMemoryReplayEventLog::new(),
        })
    }

    /// Atomically resolve and persist the replay evidence family for one scope.
    ///
    /// The first call wins permanently. `force_replay_events` is used by producers that own the
    /// canonical typed event stream. Otherwise the decision is made from canonical-event evidence
    /// visible in the same transaction, falling back to display messages.
    ///
    /// # Errors
    ///
    /// Returns a store error when the selection cannot be persisted or decoded.
    pub fn resolve_replay_source(
        &self,
        scope: &ReplayScope,
        force_replay_events: bool,
    ) -> SessionStoreResult<DurableReplaySource> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(crate::sqlite::map_sqlite_session_error)?;
        let preferred = if force_replay_events {
            DurableReplaySource::ReplayEvents
        } else {
            let has_replay_events = transaction
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM replay_events WHERE scope = ?1)",
                    [scope.as_str()],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(crate::sqlite::map_sqlite_session_error)?;
            if has_replay_events {
                DurableReplaySource::ReplayEvents
            } else {
                DurableReplaySource::DisplayMessages
            }
        };
        transaction
            .execute(
                "INSERT OR IGNORE INTO replay_source_selections (scope, source, selected_at)
                 VALUES (?1, ?2, ?3)",
                params![scope.as_str(), preferred.as_str(), Utc::now().to_rfc3339()],
            )
            .map_err(crate::sqlite::map_sqlite_session_error)?;
        let selected = transaction
            .query_row(
                "SELECT source FROM replay_source_selections WHERE scope = ?1",
                [scope.as_str()],
                |row| row.get::<_, String>(0),
            )
            .map_err(crate::sqlite::map_sqlite_session_error)?;
        transaction
            .commit()
            .map_err(crate::sqlite::map_sqlite_session_error)?;
        DurableReplaySource::from_str(&selected)
    }

    /// Atomically acquire the machine-local one-active-run admission lease.
    ///
    /// # Errors
    ///
    /// Returns a conflict for an occupied/fenced session or a storage error.
    pub fn acquire_run_admission(
        &self,
        request: AcquireRunAdmission,
    ) -> SessionStoreResult<RunAdmissionReceipt> {
        self.session_store().acquire_run_admission_sync(request)
    }

    /// Extend a run admission lease.
    ///
    /// # Errors
    ///
    /// Returns a conflict when the lease is no longer the fenced owner.
    pub fn heartbeat_run_admission(
        &self,
        lease: &RunAdmissionLease,
        lease_expires_at: DateTime<Utc>,
    ) -> SessionStoreResult<RunAdmissionLease> {
        self.session_store()
            .heartbeat_run_admission_sync(lease, lease_expires_at)
    }

    /// Release a run admission lease after durable terminal or waiting evidence is committed.
    ///
    /// # Errors
    ///
    /// Returns a conflict when the lease is no longer the fenced owner.
    pub fn release_run_admission(&self, lease: &RunAdmissionLease) -> SessionStoreResult<()> {
        self.session_store().release_run_admission_sync(lease)
    }

    /// Acquire the deletion fence for one shared session.
    ///
    /// # Errors
    ///
    /// Returns a revision, ownership, active-run, or storage conflict.
    #[allow(clippy::too_many_arguments)]
    pub fn acquire_session_deletion_fence(
        &self,
        session_id: &starweaver_core::SessionId,
        expected_revision: u64,
        fence_id: &str,
        requested_by: &str,
        idempotency_key: &str,
        command_fingerprint: &str,
    ) -> SessionStoreResult<SessionRecord> {
        self.session_store().acquire_session_deletion_fence_sync(
            session_id,
            expected_revision,
            fence_id,
            requested_by,
            idempotency_key,
            command_fingerprint,
        )
    }

    /// Tombstone a deletion-fenced shared session while retaining its evidence.
    ///
    /// # Errors
    ///
    /// Returns a conflict when the fence does not match or work remains active.
    pub fn tombstone_session(
        &self,
        session_id: &starweaver_core::SessionId,
        fence_id: &str,
    ) -> SessionStoreResult<SessionRecord> {
        self.session_store()
            .tombstone_session_sync(session_id, fence_id)
    }

    /// Return a session-store adapter backed by this resource.
    #[must_use]
    pub fn session_store(&self) -> SqliteSessionStore {
        SqliteSessionStore::from_shared(Arc::clone(&self.connection))
    }

    /// Return a stream-archive adapter backed by this resource.
    #[must_use]
    pub fn stream_archive(&self) -> SqliteStreamArchive {
        SqliteStreamArchive::from_shared(Arc::clone(&self.connection))
    }

    /// Return a replay-event-log adapter backed by this resource.
    #[must_use]
    pub fn replay_event_log(&self) -> SqliteReplayEventLog {
        SqliteReplayEventLog::from_shared(Arc::clone(&self.connection), self.live_replay.clone())
    }

    pub(crate) fn lock(&self) -> SessionStoreResult<MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|error| SessionStoreError::Failed(error.to_string()))
    }
}
