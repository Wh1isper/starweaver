use rusqlite::{Connection, OptionalExtension, params};
use starweaver_core::{RunId, SessionId};
use starweaver_session::{
    RunRecord, RunStatus, SessionRecord, SessionStoreError, SessionStoreResult,
};

use crate::sqlite::{
    collect_json_record_rows, deserialize_json_record, format_run_key, map_display_session_error,
    map_sqlite_session_error, serialize_json_record,
};

pub fn save_session_record(
    connection: &Connection,
    session: &SessionRecord,
) -> SessionStoreResult<()> {
    connection
        .execute(
            "INSERT INTO session_records (session_id, record, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(session_id) DO UPDATE SET
               record = excluded.record,
               created_at = excluded.created_at,
               updated_at = excluded.updated_at",
            params![
                session.session_id.as_str(),
                serialize_json_record(session)?,
                session.created_at.to_rfc3339(),
                session.updated_at.to_rfc3339(),
            ],
        )
        .map_err(map_sqlite_session_error)?;
    Ok(())
}

pub fn load_session_record(
    connection: &Connection,
    session_id: &SessionId,
) -> SessionStoreResult<SessionRecord> {
    let payload = connection
        .query_row(
            "SELECT record FROM session_records WHERE session_id = ?1",
            params![session_id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(map_sqlite_session_error)?
        .ok_or_else(|| SessionStoreError::NotFound(session_id.as_str().to_string()))?;
    deserialize_json_record(&payload)
}

pub fn allocate_or_reuse_run_sequence(
    connection: &Connection,
    run: &mut RunRecord,
) -> SessionStoreResult<Option<RunRecord>> {
    let existing = connection
        .query_row(
            "SELECT record FROM run_records WHERE session_id = ?1 AND run_id = ?2",
            params![run.session_id.as_str(), run.run_id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(map_sqlite_session_error)?
        .map(|payload| deserialize_json_record::<RunRecord>(&payload))
        .transpose()?;
    if let Some(persisted) = existing {
        if run.sequence_no != 0 && run.sequence_no != persisted.sequence_no {
            return Err(SessionStoreError::Failed(format!(
                "run sequence is immutable for session {} and run {}: persisted {}, received {}",
                run.session_id.as_str(),
                run.run_id.as_str(),
                persisted.sequence_no,
                run.sequence_no
            )));
        }
        run.sequence_no = persisted.sequence_no;
        return Ok(Some(persisted));
    }
    if run.sequence_no == 0 {
        let next_sequence: i64 = connection
            .query_row(
                "SELECT COALESCE(MAX(sequence_no), 0) + 1 FROM run_records WHERE session_id = ?1",
                params![run.session_id.as_str()],
                |row| row.get(0),
            )
            .map_err(map_sqlite_session_error)?;
        run.sequence_no = usize::try_from(next_sequence).map_err(map_display_session_error)?;
    }
    Ok(None)
}

pub fn save_run_record(connection: &Connection, run: &RunRecord) -> SessionStoreResult<()> {
    connection
        .execute(
            "INSERT INTO run_records
             (session_id, run_id, record, sequence_no, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(session_id, run_id) DO UPDATE SET
               record = excluded.record,
               sequence_no = excluded.sequence_no,
               created_at = excluded.created_at,
               updated_at = excluded.updated_at",
            params![
                run.session_id.as_str(),
                run.run_id.as_str(),
                serialize_json_record(run)?,
                i64::try_from(run.sequence_no).map_err(map_display_session_error)?,
                run.created_at.to_rfc3339(),
                run.updated_at.to_rfc3339(),
            ],
        )
        .map_err(map_sqlite_session_error)?;
    Ok(())
}

pub fn load_run_record(
    connection: &Connection,
    session_id: &SessionId,
    run_id: &RunId,
) -> SessionStoreResult<RunRecord> {
    let payload = connection
        .query_row(
            "SELECT record FROM run_records WHERE session_id = ?1 AND run_id = ?2",
            params![session_id.as_str(), run_id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(map_sqlite_session_error)?
        .ok_or_else(|| SessionStoreError::NotFound(format_run_key(session_id, run_id)))?;
    deserialize_json_record(&payload)
}

pub fn list_run_records(
    connection: &Connection,
    session_id: &SessionId,
) -> SessionStoreResult<Vec<RunRecord>> {
    let mut statement = connection
        .prepare("SELECT record FROM run_records WHERE session_id = ?1 ORDER BY sequence_no ASC")
        .map_err(map_sqlite_session_error)?;
    let rows = statement
        .query_map(params![session_id.as_str()], |row| row.get::<_, String>(0))
        .map_err(map_sqlite_session_error)?;
    collect_json_record_rows(rows)
}

pub fn apply_run_to_session(session: &mut SessionRecord, run: &RunRecord) {
    session.head_run_id = Some(run.run_id.clone());
    if run.status.is_active() {
        session.active_run_id = Some(run.run_id.clone());
    } else {
        if run.status == RunStatus::Completed {
            session.head_success_run_id = Some(run.run_id.clone());
        }
        if session.active_run_id.as_ref() == Some(&run.run_id) {
            session.active_run_id = None;
        }
    }
    session.updated_at = run.updated_at;
}
