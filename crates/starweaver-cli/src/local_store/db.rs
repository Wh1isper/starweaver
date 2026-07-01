use std::{fs, path::Path};

use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use starweaver_agent::ResumableState;
use starweaver_core::{CheckpointId, RunId};
use starweaver_environment::EnvironmentState;
use starweaver_runtime::{AgentStreamEvent, AgentStreamRecord};
use starweaver_session::{
    ApprovalRecord, CheckpointRef, DeferredToolRecord, RunRecord, RunStatus, SessionRecord,
    SessionStatus, StreamCursorRef,
};
use starweaver_stream::DisplayMessage;
use uuid::Uuid;

use super::FileRefRecord;
use crate::{CliError, CliResult, error::io_error};

pub(super) fn add_column_if_missing(
    conn: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> CliResult<()> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    if !columns.iter().any(|existing| existing == column) {
        conn.execute(
            &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
            [],
        )?;
    }
    Ok(())
}

pub(super) fn load_session_tx(
    tx: &rusqlite::Transaction<'_>,
    session_id: &str,
) -> CliResult<SessionRecord> {
    tx.query_row(
        "SELECT record_json FROM sessions WHERE session_id = ?1",
        [session_id],
        |row| row.get::<_, String>(0),
    )
    .optional()?
    .map(|json| serde_json::from_str(&json).map_err(CliError::from))
    .transpose()?
    .ok_or_else(|| CliError::NotFound(session_id.to_string()))
}

pub(super) fn next_sequence_tx(
    tx: &rusqlite::Transaction<'_>,
    session_id: &str,
) -> CliResult<usize> {
    let value = tx.query_row(
        "SELECT COALESCE(MAX(sequence_no), 0) + 1 FROM runs WHERE session_id = ?1",
        [session_id],
        |row| row.get::<_, i64>(0),
    )?;
    usize::try_from(value).map_err(|error| CliError::Storage(error.to_string()))
}

pub(super) fn upsert_session_tx(
    tx: &rusqlite::Transaction<'_>,
    session: &SessionRecord,
) -> CliResult<()> {
    tx.execute(
        r"
        INSERT INTO sessions (session_id, status, profile, title, head_run_id, head_success_run_id, active_run_id, created_at, updated_at, record_json)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
        ON CONFLICT(session_id) DO UPDATE SET
            status = excluded.status,
            profile = excluded.profile,
            title = excluded.title,
            head_run_id = excluded.head_run_id,
            head_success_run_id = excluded.head_success_run_id,
            active_run_id = excluded.active_run_id,
            updated_at = excluded.updated_at,
            record_json = excluded.record_json
        ",
        params![
            session.session_id.as_str(),
            session_status(session.status),
            session.profile.as_deref(),
            session.title.as_deref(),
            session.head_run_id.as_ref().map(RunId::as_str),
            session.head_success_run_id.as_ref().map(RunId::as_str),
            session.active_run_id.as_ref().map(RunId::as_str),
            session.created_at.to_rfc3339(),
            session.updated_at.to_rfc3339(),
            serde_json::to_string(session)?,
        ],
    )?;
    Ok(())
}

pub(super) fn upsert_run_tx(tx: &rusqlite::Transaction<'_>, run: &RunRecord) -> CliResult<()> {
    tx.execute(
        r"
        INSERT INTO runs (session_id, run_id, sequence_no, status, restore_from_run_id, output_preview, created_at, updated_at, record_json)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
        ON CONFLICT(session_id, run_id) DO UPDATE SET
            sequence_no = excluded.sequence_no,
            status = excluded.status,
            restore_from_run_id = excluded.restore_from_run_id,
            output_preview = excluded.output_preview,
            updated_at = excluded.updated_at,
            record_json = excluded.record_json
        ",
        params![
            run.session_id.as_str(),
            run.run_id.as_str(),
            usize_to_i64(run.sequence_no)?,
            run_status(run.status),
            run.restore_from_run_id.as_ref().map(RunId::as_str),
            run.output_preview.as_deref(),
            run.created_at.to_rfc3339(),
            run.updated_at.to_rfc3339(),
            serde_json::to_string(run)?,
        ],
    )?;
    Ok(())
}

pub(super) fn insert_raw_stream_records_tx(
    tx: &rusqlite::Transaction<'_>,
    run: &RunRecord,
    records: &[AgentStreamRecord],
) -> CliResult<()> {
    for record in records {
        tx.execute(
            r"
            INSERT OR REPLACE INTO raw_stream_records (session_id, run_id, sequence_no, kind, created_at, record_json)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ",
            params![
                run.session_id.as_str(),
                run.run_id.as_str(),
                usize_to_i64(record.sequence)?,
                raw_stream_kind(&record.event),
                Utc::now().to_rfc3339(),
                serde_json::to_string(record)?,
            ],
        )?;
    }
    Ok(())
}

pub(super) fn insert_display_messages_tx(
    tx: &rusqlite::Transaction<'_>,
    messages: &[DisplayMessage],
) -> CliResult<()> {
    for message in messages {
        tx.execute(
            r"
            INSERT OR REPLACE INTO display_messages (session_id, run_id, sequence_no, kind, created_at, message_json)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ",
            params![
                message.session_id.as_str(),
                message.run_id.as_str(),
                usize_to_i64(message.sequence)?,
                format!("{:?}", message.kind).to_lowercase(),
                message.timestamp.to_rfc3339(),
                serde_json::to_string(message)?,
            ],
        )?;
    }
    Ok(())
}

pub(super) fn insert_context_state_tx(
    tx: &rusqlite::Transaction<'_>,
    run: &RunRecord,
    state: &ResumableState,
) -> CliResult<()> {
    tx.execute(
        "INSERT OR REPLACE INTO context_states (session_id, run_id, state_json, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![
            run.session_id.as_str(),
            run.run_id.as_str(),
            serde_json::to_string(state)?,
            Utc::now().to_rfc3339(),
        ],
    )?;
    Ok(())
}

pub(super) fn insert_environment_state_tx(
    tx: &rusqlite::Transaction<'_>,
    run: &RunRecord,
    state: &EnvironmentState,
) -> CliResult<()> {
    let ref_id = format!(
        "{}:{}:environment",
        run.session_id.as_str(),
        run.run_id.as_str()
    );
    tx.execute(
        "INSERT OR REPLACE INTO environment_states (ref_id, session_id, run_id, provider, state_json, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            ref_id,
            run.session_id.as_str(),
            run.run_id.as_str(),
            state.provider_id,
            serde_json::to_string(state)?,
            Utc::now().to_rfc3339(),
        ],
    )?;
    Ok(())
}

pub(super) fn insert_stream_cursor_tx(
    tx: &rusqlite::Transaction<'_>,
    run: &RunRecord,
    cursor: &StreamCursorRef,
) -> CliResult<()> {
    tx.execute(
        "INSERT OR REPLACE INTO stream_cursors (session_id, run_id, family, scope, sequence_no, cursor_json, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            run.session_id.as_str(),
            run.run_id.as_str(),
            cursor.family,
            cursor.scope,
            usize_to_i64(cursor.sequence)?,
            serde_json::to_string(cursor)?,
            cursor.created_at.to_rfc3339(),
        ],
    )?;
    Ok(())
}

pub(super) fn insert_checkpoint_refs_tx(
    tx: &rusqlite::Transaction<'_>,
    run: &RunRecord,
    checkpoints: &[CheckpointRef],
) -> CliResult<()> {
    for checkpoint in checkpoints {
        tx.execute(
            "INSERT OR REPLACE INTO checkpoints (checkpoint_id, session_id, run_id, sequence_no, node, checkpoint_json, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                checkpoint.checkpoint_id.as_str(),
                run.session_id.as_str(),
                run.run_id.as_str(),
                usize_to_i64(checkpoint.sequence)?,
                checkpoint.node,
                serde_json::to_string(checkpoint)?,
                checkpoint.created_at.to_rfc3339(),
            ],
        )?;
    }
    Ok(())
}

pub(super) fn insert_approval_records_tx(
    tx: &rusqlite::Transaction<'_>,
    approvals: &[ApprovalRecord],
) -> CliResult<()> {
    for approval in approvals {
        tx.execute(
            "INSERT OR REPLACE INTO approvals (approval_id, session_id, run_id, action_id, action_name, status, record_json, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                approval.approval_id,
                approval.session_id.as_str(),
                approval.run_id.as_str(),
                approval.action_id,
                approval.action_name,
                format!("{:?}", approval.status).to_lowercase(),
                serde_json::to_string(approval)?,
                approval.created_at.to_rfc3339(),
                approval.updated_at.to_rfc3339(),
            ],
        )?;
    }
    Ok(())
}

pub(super) fn insert_deferred_tool_records_tx(
    tx: &rusqlite::Transaction<'_>,
    deferred_tools: &[DeferredToolRecord],
) -> CliResult<()> {
    for deferred in deferred_tools {
        tx.execute(
            "INSERT OR REPLACE INTO deferred_tools (deferred_id, session_id, run_id, tool_call_id, tool_name, status, record_json, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                deferred.deferred_id,
                deferred.session_id.as_str(),
                deferred.run_id.as_str(),
                deferred.tool_call_id,
                deferred.tool_name,
                format!("{:?}", deferred.status).to_lowercase(),
                serde_json::to_string(deferred)?,
                deferred.created_at.to_rfc3339(),
                deferred.updated_at.to_rfc3339(),
            ],
        )?;
    }
    Ok(())
}

pub(super) fn insert_file_ref_tx(
    tx: &rusqlite::Transaction<'_>,
    run: &RunRecord,
    file_ref: &FileRefRecord,
) -> CliResult<()> {
    tx.execute(
        "INSERT OR REPLACE INTO file_refs (ref_id, session_id, run_id, relative_path, byte_size, checksum, content_type, created_at, trimmed_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL)",
        params![
            file_ref.ref_id,
            run.session_id.as_str(),
            run.run_id.as_str(),
            file_ref.relative_path,
            file_ref.byte_size,
            file_ref.checksum,
            file_ref.content_type,
            file_ref.created_at,
        ],
    )?;
    Ok(())
}

pub(super) fn checkpoint_refs(
    run: &RunRecord,
    records: &[AgentStreamRecord],
) -> Vec<CheckpointRef> {
    records
        .iter()
        .filter_map(|record| match &record.event {
            AgentStreamEvent::Checkpoint { node, step } => Some(CheckpointRef {
                checkpoint_id: CheckpointId::from_string(format!(
                    "ckpt_{}_{}",
                    run.run_id.as_str(),
                    record.sequence
                )),
                run_id: run.run_id.clone(),
                sequence: record.sequence,
                node: format!("{node:?}"),
                storage_ref: Some(format!(
                    "sessions/{}/runs/{}/checkpoints/{}.json",
                    run.session_id.as_str(),
                    run.run_id.as_str(),
                    record.sequence
                )),
                stream_cursor: Some(record.sequence),
                created_at: Utc::now(),
                metadata: serde_json::json!({"step": step})
                    .as_object()
                    .cloned()
                    .unwrap_or_default(),
            }),
            _ => None,
        })
        .collect()
}

pub(super) const fn raw_stream_kind(event: &AgentStreamEvent) -> &'static str {
    match event {
        AgentStreamEvent::RunStart { .. } => "run_start",
        AgentStreamEvent::NodeStart { .. } => "node_start",
        AgentStreamEvent::NodeComplete { .. } => "node_complete",
        AgentStreamEvent::Custom { .. } => "custom",
        AgentStreamEvent::ModelRequest { .. } => "model_request",
        AgentStreamEvent::ModelStream { .. } => "model_stream",
        AgentStreamEvent::ModelResponse { .. } => "model_response",
        AgentStreamEvent::Checkpoint { .. } => "checkpoint",
        AgentStreamEvent::Suspended { .. } => "suspended",
        AgentStreamEvent::ToolCall { .. } => "tool_call",
        AgentStreamEvent::ToolReturn { .. } => "tool_return",
        AgentStreamEvent::OutputRetry { .. } => "output_retry",
        AgentStreamEvent::SteeringGuard { .. } => "steering_guard",
        AgentStreamEvent::RunComplete { .. } => "run_complete",
        AgentStreamEvent::RunFailed { .. } => "run_failed",
    }
}

pub(super) const fn session_status(status: SessionStatus) -> &'static str {
    match status {
        SessionStatus::Active => "active",
        SessionStatus::Archived => "archived",
        SessionStatus::Failed => "failed",
    }
}

pub(super) const fn run_status(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Queued => "queued",
        RunStatus::Running => "running",
        RunStatus::Waiting => "waiting",
        RunStatus::Completed => "completed",
        RunStatus::Failed => "failed",
        RunStatus::Cancelled => "cancelled",
    }
}

pub(super) fn usize_to_i64(value: usize) -> CliResult<i64> {
    i64::try_from(value).map_err(|error| CliError::Storage(error.to_string()))
}

pub(super) fn i64_to_usize(value: i64) -> rusqlite::Result<usize> {
    usize::try_from(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })
}

pub(super) fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> CliResult<()> {
    let parent = path
        .parent()
        .ok_or_else(|| CliError::Storage("missing parent path".to_string()))?;
    fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
    let temp = path.with_extension(format!("{}.tmp", Uuid::new_v4()));
    fs::write(&temp, serde_json::to_vec_pretty(value)?).map_err(|error| io_error(&temp, error))?;
    fs::rename(&temp, path).map_err(|error| io_error(path, error))?;
    Ok(())
}

pub(super) fn cheap_checksum(data: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in data {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("fnv64:{hash:016x}")
}
