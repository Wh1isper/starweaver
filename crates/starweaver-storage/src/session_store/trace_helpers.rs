use rusqlite::{Connection, params};
use starweaver_context::AgentCheckpoint;
use starweaver_core::{CheckpointId, RunId, SessionId};
use starweaver_session::{ApprovalRecord, ApprovalStatus, SessionStoreResult};

use crate::sqlite::{
    collect_json_record_rows, map_display_session_error, map_sqlite_session_error,
};

pub(super) fn latest_stream_sequence(
    connection: &Connection,
    session_id: &SessionId,
    run_id: &RunId,
) -> SessionStoreResult<Option<usize>> {
    let value = connection
        .query_row(
            "SELECT MAX(sequence_no) FROM stream_records WHERE session_id = ?1 AND run_id = ?2",
            params![session_id.as_str(), run_id.as_str()],
            |row| row.get::<_, Option<i64>>(0),
        )
        .map_err(map_sqlite_session_error)?;
    value
        .map(|sequence| usize::try_from(sequence).map_err(map_display_session_error))
        .transpose()
}

pub(super) fn load_checkpoint_ids(
    connection: &Connection,
    session_id: &SessionId,
    run_id: &RunId,
) -> SessionStoreResult<Vec<CheckpointId>> {
    let mut statement = connection
        .prepare(
            "SELECT record FROM checkpoint_records
             WHERE session_id = ?1 AND run_id = ?2
             ORDER BY sequence_no ASC, created_at ASC, rowid ASC",
        )
        .map_err(map_sqlite_session_error)?;
    let rows = statement
        .query_map(params![session_id.as_str(), run_id.as_str()], |row| {
            row.get::<_, String>(0)
        })
        .map_err(map_sqlite_session_error)?;
    let checkpoints = collect_json_record_rows::<AgentCheckpoint>(rows)?;
    Ok(checkpoints
        .into_iter()
        .map(|checkpoint| checkpoint.checkpoint_id)
        .collect())
}

pub(super) fn count_pending_approvals(
    connection: &Connection,
    session_id: &SessionId,
    run_id: &RunId,
) -> SessionStoreResult<usize> {
    let approvals = {
        let mut statement = connection
            .prepare("SELECT record FROM approval_records WHERE session_id = ?1 AND run_id = ?2")
            .map_err(map_sqlite_session_error)?;
        let rows = statement
            .query_map(params![session_id.as_str(), run_id.as_str()], |row| {
                row.get::<_, String>(0)
            })
            .map_err(map_sqlite_session_error)?;
        collect_json_record_rows::<ApprovalRecord>(rows)?
    };
    Ok(approvals
        .iter()
        .filter(|approval| approval.status == ApprovalStatus::Pending)
        .count())
}

pub(super) fn count_deferred_tools(
    connection: &Connection,
    session_id: &SessionId,
    run_id: &RunId,
) -> SessionStoreResult<usize> {
    let count = connection
        .query_row(
            "SELECT COUNT(*) FROM deferred_tool_records WHERE session_id = ?1 AND run_id = ?2",
            params![session_id.as_str(), run_id.as_str()],
            |row| row.get::<_, i64>(0),
        )
        .map_err(map_sqlite_session_error)?;
    usize::try_from(count).map_err(map_display_session_error)
}
