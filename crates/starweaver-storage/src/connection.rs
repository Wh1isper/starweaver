//! SQLite connection helpers.

use std::path::Path;

use rusqlite::Connection;
use starweaver_session::SessionStoreResult;

use crate::errors::sql_error;

pub fn open_connection(path: impl AsRef<Path>) -> SessionStoreResult<Connection> {
    Connection::open(path).map_err(sql_error)
}

pub fn open_in_memory_connection() -> SessionStoreResult<Connection> {
    Connection::open_in_memory().map_err(sql_error)
}
