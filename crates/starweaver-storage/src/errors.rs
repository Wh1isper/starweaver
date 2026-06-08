//! Shared SQLite adapter error and JSON helpers.

use starweaver_core::{RunId, SessionId};
use starweaver_session::{SessionStoreError, SessionStoreResult};
use starweaver_stream::ReplayError;

pub fn collect_json_rows<T>(
    rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<String>>,
) -> SessionStoreResult<Vec<T>>
where
    T: serde::de::DeserializeOwned,
{
    let mut values = Vec::new();
    for row in rows {
        values.push(deserialize(&row.map_err(sql_error)?)?);
    }
    Ok(values)
}

pub fn serialize<T>(value: &T) -> SessionStoreResult<String>
where
    T: serde::Serialize,
{
    serde_json::to_string(value).map_err(|error| SessionStoreError::Failed(error.to_string()))
}

pub fn deserialize<T>(payload: &str) -> SessionStoreResult<T>
where
    T: serde::de::DeserializeOwned,
{
    serde_json::from_str(payload).map_err(|error| SessionStoreError::Failed(error.to_string()))
}

pub fn sql_error(error: rusqlite::Error) -> SessionStoreError {
    SessionStoreError::Failed(error.to_string())
}

pub fn int_error(error: impl std::fmt::Display) -> SessionStoreError {
    SessionStoreError::Failed(error.to_string())
}

pub fn run_key_label(session_id: &SessionId, run_id: &RunId) -> String {
    format!("{}:{}", session_id.as_str(), run_id.as_str())
}

pub fn session_to_replay_error(error: SessionStoreError) -> ReplayError {
    ReplayError::Failed(error.to_string())
}

pub fn replay_sql_error(error: rusqlite::Error) -> ReplayError {
    ReplayError::Failed(error.to_string())
}
