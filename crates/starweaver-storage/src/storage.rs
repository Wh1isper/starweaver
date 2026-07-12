//! Unified SQLite storage resource and adapter construction.

use std::{
    path::Path,
    sync::{Arc, MutexGuard},
    time::Duration,
};

use rusqlite::Connection;
use starweaver_session::{SessionStoreError, SessionStoreResult};
use starweaver_stream::InMemoryReplayEventLog;

use crate::{
    migrations::apply_sqlite_migrations,
    replay_log::SqliteReplayEventLog,
    session_store::SqliteSessionStore,
    sqlite::{SharedSqliteConnection, open_in_memory_sqlite_connection, open_sqlite_connection},
    stream_archive::SqliteStreamArchive,
};

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
