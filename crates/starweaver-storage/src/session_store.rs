//! SQLite-backed durable session store adapter.

use std::{
    path::Path,
    sync::{Arc, Mutex, MutexGuard},
};

use rusqlite::Connection;
use starweaver_session::{SessionStoreError, SessionStoreResult};

use crate::{
    migrations::apply_sqlite_migrations,
    sqlite::{open_in_memory_sqlite_connection, open_sqlite_connection},
};

mod impl_store;
mod records;
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
        let mut connection = open_sqlite_connection(path)?;
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
        let mut connection = open_in_memory_sqlite_connection()?;
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
