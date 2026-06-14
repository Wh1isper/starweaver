//! Shared SQLite connection, JSON, and error mapping helpers.

use std::path::Path;

use rusqlite::Connection;
use starweaver_core::{RunId, SessionId};
use starweaver_session::{SessionStoreError, SessionStoreResult};
use starweaver_stream::ReplayError;

pub fn open_sqlite_connection(path: impl AsRef<Path>) -> SessionStoreResult<Connection> {
    Connection::open(path).map_err(map_sqlite_session_error)
}

pub fn open_in_memory_sqlite_connection() -> SessionStoreResult<Connection> {
    Connection::open_in_memory().map_err(map_sqlite_session_error)
}

pub fn collect_json_record_rows<T>(
    rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<String>>,
) -> SessionStoreResult<Vec<T>>
where
    T: serde::de::DeserializeOwned,
{
    let mut values = Vec::new();
    for row in rows {
        values.push(deserialize_json_record(
            &row.map_err(map_sqlite_session_error)?,
        )?);
    }
    Ok(values)
}

pub fn serialize_json_record<T>(value: &T) -> SessionStoreResult<String>
where
    T: serde::Serialize,
{
    serde_json::to_string(value).map_err(|error| SessionStoreError::Failed(error.to_string()))
}

pub fn deserialize_json_record<T>(payload: &str) -> SessionStoreResult<T>
where
    T: serde::de::DeserializeOwned,
{
    serde_json::from_str(payload).map_err(|error| SessionStoreError::Failed(error.to_string()))
}

pub fn map_sqlite_session_error(error: rusqlite::Error) -> SessionStoreError {
    SessionStoreError::Failed(error.to_string())
}

pub fn map_display_session_error(error: impl std::fmt::Display) -> SessionStoreError {
    SessionStoreError::Failed(error.to_string())
}

pub fn format_run_key(session_id: &SessionId, run_id: &RunId) -> String {
    format!("{}:{}", session_id.as_str(), run_id.as_str())
}

pub fn map_session_to_replay_error(error: SessionStoreError) -> ReplayError {
    ReplayError::Failed(error.to_string())
}

pub fn map_sqlite_replay_error(error: rusqlite::Error) -> ReplayError {
    ReplayError::Failed(error.to_string())
}
