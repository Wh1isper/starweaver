//! SQLite-backed durable session store adapter.

use std::{
    path::Path,
    sync::{Arc, Mutex, MutexGuard},
};

use rusqlite::Connection;
use starweaver_session::{SessionStoreError, SessionStoreResult};

use crate::{SqliteStorage, sqlite::SharedSqliteConnection};

mod impl_store;
mod management;
pub mod records;
mod trace_helpers;

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
        Ok(SqliteStorage::open(path)?.session_store())
    }

    /// Open an in-memory SQLite session store.
    ///
    /// # Errors
    ///
    /// Returns a store error when SQLite cannot initialize the database.
    pub fn in_memory() -> SessionStoreResult<Self> {
        Ok(SqliteStorage::in_memory()?.session_store())
    }

    pub(crate) const fn from_shared(connection: SharedSqliteConnection) -> Self {
        Self { connection }
    }

    fn lock(&self) -> SessionStoreResult<MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|error| SessionStoreError::Failed(error.to_string()))
    }
}
