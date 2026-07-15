//! SQLite migration application and status reporting.

use std::{collections::BTreeSet, path::Path};

use chrono::Utc;
use rusqlite::{Connection, TransactionBehavior, params};
use starweaver_context::AgentCheckpoint;
use starweaver_core::{from_versioned_json, to_versioned_json};
use starweaver_session::{CheckpointRef, SessionStoreError, SessionStoreResult};
use starweaver_stream::{
    DisplayMessage, ReplayCursorFamily, ReplayEvent, ReplayEventKind, ReplayScope, ReplaySnapshot,
};

use crate::{
    schema::{SQLITE_MIGRATIONS, SQLITE_SCHEMA_MIGRATION_TABLE},
    sqlite::map_sqlite_session_error,
};

/// Applied SQLite schema migration metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SqliteAppliedMigration {
    /// Migration id.
    pub id: String,
    /// Migration description.
    pub description: String,
    /// Migration SQL checksum when recorded by the database.
    pub checksum: Option<String>,
    /// Application timestamp if recorded by the database.
    pub applied_at: Option<String>,
}

/// Pending SQLite schema migration metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SqlitePendingMigration {
    /// Migration id.
    pub id: &'static str,
    /// Migration description.
    pub description: &'static str,
    /// Migration SQL checksum.
    pub checksum: String,
}

/// SQLite schema migration status.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SqliteMigrationStatus {
    /// True when the migration tracking table exists.
    pub migration_table_exists: bool,
    /// Applied migrations recorded by the database.
    pub applied: Vec<SqliteAppliedMigration>,
    /// Workspace migrations still pending.
    pub pending: Vec<SqlitePendingMigration>,
    /// Latest known migration id.
    pub latest_migration: Option<&'static str>,
    /// True when every known migration has been applied with a valid checksum.
    pub current: bool,
}

impl SqliteMigrationStatus {
    /// Return true when every recorded known migration checksum matches the workspace SQL.
    #[must_use]
    pub fn checksums_valid(&self) -> bool {
        applied_migration_checksums_valid(&self.applied)
    }
}

/// Run all pending SQLite schema migrations for a database file.
///
/// # Errors
///
/// Returns a store error when SQLite cannot open the database or apply a migration.
pub fn migrate_sqlite_database(path: impl AsRef<Path>) -> SessionStoreResult<Vec<&'static str>> {
    let mut connection = Connection::open(path).map_err(map_sqlite_session_error)?;
    apply_sqlite_migrations(&mut connection)
}

/// Inspect SQLite schema migration status without applying migrations.
///
/// # Errors
///
/// Returns a store error when SQLite cannot open or inspect the database.
pub fn sqlite_migration_status(
    path: impl AsRef<Path>,
) -> SessionStoreResult<SqliteMigrationStatus> {
    let connection = Connection::open(path).map_err(map_sqlite_session_error)?;
    sqlite_migration_status_for_connection(&connection)
}

fn sqlite_migration_status_for_connection(
    connection: &Connection,
) -> SessionStoreResult<SqliteMigrationStatus> {
    let migration_table_exists = table_exists(connection, SQLITE_SCHEMA_MIGRATION_TABLE)?;
    let applied = if migration_table_exists {
        load_applied_migration_records(connection)?
    } else {
        Vec::new()
    };
    let applied_ids = applied
        .iter()
        .map(|migration| migration.id.as_str())
        .collect::<BTreeSet<_>>();
    let pending = SQLITE_MIGRATIONS
        .iter()
        .filter(|migration| !applied_ids.contains(migration.id))
        .map(|migration| SqlitePendingMigration {
            id: migration.id,
            description: migration.description,
            checksum: migration.checksum(),
        })
        .collect::<Vec<_>>();
    let checksums_valid = applied_migration_checksums_valid(&applied);
    Ok(SqliteMigrationStatus {
        migration_table_exists,
        applied,
        pending: pending.clone(),
        latest_migration: SQLITE_MIGRATIONS.last().map(|migration| migration.id),
        current: pending.is_empty() && checksums_valid,
    })
}

fn load_applied_migration_records(
    connection: &Connection,
) -> SessionStoreResult<Vec<SqliteAppliedMigration>> {
    let mut statement = connection
        .prepare(&format!(
            "SELECT id, description, checksum, applied_at FROM {SQLITE_SCHEMA_MIGRATION_TABLE} ORDER BY applied_at ASC, id ASC"
        ))
        .map_err(map_sqlite_session_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok(SqliteAppliedMigration {
                id: row.get::<_, String>(0)?,
                description: row.get::<_, String>(1)?,
                checksum: row.get::<_, Option<String>>(2)?,
                applied_at: row.get::<_, Option<String>>(3)?,
            })
        })
        .map_err(map_sqlite_session_error)?;
    let mut migrations = Vec::new();
    for row in rows {
        migrations.push(row.map_err(map_sqlite_session_error)?);
    }
    Ok(migrations)
}

pub fn apply_sqlite_migrations(
    connection: &mut Connection,
) -> SessionStoreResult<Vec<&'static str>> {
    connection
        .execute_batch(
            r"
            PRAGMA journal_mode = WAL;
            PRAGMA foreign_keys = ON;
            ",
        )
        .map_err(map_sqlite_session_error)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(map_sqlite_session_error)?;
    transaction
        .execute(
            &format!(
                "CREATE TABLE IF NOT EXISTS {SQLITE_SCHEMA_MIGRATION_TABLE} (
                    id TEXT PRIMARY KEY,
                    description TEXT NOT NULL,
                    checksum TEXT,
                    applied_at TEXT NOT NULL
                )"
            ),
            [],
        )
        .map_err(map_sqlite_session_error)?;
    ensure_migration_checksum_column(&transaction)?;
    validate_and_backfill_migration_checksums(&transaction)?;
    let applied = load_applied_migrations(&transaction)?;
    let mut newly_applied = Vec::new();
    for migration in SQLITE_MIGRATIONS {
        if applied.contains(migration.id) {
            continue;
        }
        transaction
            .execute_batch(migration.sql)
            .map_err(map_sqlite_session_error)?;
        if migration.id == "20260711_000002_namespaced_evidence_tables" {
            backfill_namespaced_evidence_tables(&transaction)?;
        }
        if migration.id == "20260711_000003_split_display_and_replay_families" {
            backfill_display_message_records(&transaction)?;
        }
        if migration.id == "20260712_000004_evidence_outbox_and_resume_claims" {
            backfill_run_evidence_digests(&transaction)?;
        }
        transaction
            .execute(
                &format!(
                    "INSERT INTO {SQLITE_SCHEMA_MIGRATION_TABLE} (id, description, checksum, applied_at)
                     VALUES (?1, ?2, ?3, ?4)"
                ),
                params![
                    migration.id,
                    migration.description,
                    migration.checksum(),
                    Utc::now().to_rfc3339()
                ],
            )
            .map_err(map_sqlite_session_error)?;
        newly_applied.push(migration.id);
    }
    transaction.commit().map_err(map_sqlite_session_error)?;
    Ok(newly_applied)
}

const LEGACY_UNSEALED_EVIDENCE_DIGEST: &str = "legacy-unsealed:v1";

fn backfill_run_evidence_digests(connection: &Connection) -> SessionStoreResult<()> {
    let mut statement = connection
        .prepare("SELECT session_id, run_id, record, updated_at FROM run_records")
        .map_err(map_sqlite_session_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(map_sqlite_session_error)?;
    let mut digests = Vec::new();
    for row in rows {
        let (session_id, run_id, payload, updated_at) = row.map_err(map_sqlite_session_error)?;
        // Every pre-migration run must be sealed with an unretryable marker. Historical run
        // metadata was caller-controlled, including the former digest key, so it is never a
        // trustworthy source for an exact-retry digest. Preserve opaque payloads while preventing
        // any first post-migration bundle from adopting an existing run identity.
        let _ = payload;
        digests.push((
            session_id,
            run_id,
            LEGACY_UNSEALED_EVIDENCE_DIGEST.to_string(),
            updated_at,
        ));
    }
    drop(statement);
    for (session_id, run_id, digest, created_at) in digests {
        connection
            .execute(
                "INSERT OR IGNORE INTO run_evidence_commits
                 (session_id, run_id, digest, created_at) VALUES (?1, ?2, ?3, ?4)",
                params![session_id, run_id, digest, created_at],
            )
            .map_err(map_sqlite_session_error)?;
    }
    Ok(())
}

fn backfill_display_message_records(connection: &Connection) -> SessionStoreResult<()> {
    if table_exists(connection, "replay_events")? {
        let mut statement = connection
            .prepare(
                "SELECT scope, sequence_no, record, created_at
                 FROM replay_events
                 WHERE json_extract(record, '$.event.kind') = 'display_message'
                    OR json_extract(record, '$.payload.event.kind') = 'display_message'
                 ORDER BY scope, sequence_no",
            )
            .map_err(map_sqlite_session_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(map_sqlite_session_error)?;
        let mut records = Vec::new();
        for row in rows {
            records.push(row.map_err(map_sqlite_session_error)?);
        }
        drop(statement);

        for (scope, sequence, payload, created_at) in &records {
            let expected = from_versioned_json::<ReplayEvent>(payload)
                .map_err(|error| SessionStoreError::Failed(error.to_string()))?;
            if !matches!(&expected.event, ReplayEventKind::DisplayMessage(_)) {
                return Err(SessionStoreError::Failed(format!(
                    "display backfill selected a non-display replay event for scope {scope} at sequence {sequence}"
                )));
            }
            let canonical_payload = to_versioned_json(&expected)
                .map_err(|error| SessionStoreError::Failed(error.to_string()))?;
            let inserted = connection
                .execute(
                    "INSERT OR IGNORE INTO display_message_records
                     (scope, sequence_no, record, created_at)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![scope, sequence, canonical_payload, created_at],
                )
                .map_err(map_sqlite_session_error)?;
            if inserted == 0 {
                let persisted = connection
                    .query_row(
                        "SELECT record FROM display_message_records
                         WHERE scope = ?1 AND sequence_no = ?2",
                        params![scope, sequence],
                        |row| row.get::<_, String>(0),
                    )
                    .map_err(map_sqlite_session_error)?;
                let persisted = from_versioned_json::<ReplayEvent>(&persisted)
                    .map_err(|error| SessionStoreError::Failed(error.to_string()))?;
                if persisted != expected {
                    return Err(SessionStoreError::Failed(format!(
                        "migration payload conflict while splitting display scope {scope} at sequence {sequence}"
                    )));
                }
            }
        }
        connection
            .execute(
                "DELETE FROM replay_events
                 WHERE json_extract(record, '$.event.kind') = 'display_message'
                    OR json_extract(record, '$.payload.event.kind') = 'display_message'",
                [],
            )
            .map_err(map_sqlite_session_error)?;
    }
    move_legacy_display_snapshots(connection)
}

fn move_legacy_display_snapshots(connection: &Connection) -> SessionStoreResult<()> {
    if !table_exists(connection, "replay_snapshot_records")? {
        return Ok(());
    }
    let mut statement = connection
        .prepare("SELECT scope, record, updated_at FROM replay_snapshot_records ORDER BY scope")
        .map_err(map_sqlite_session_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(map_sqlite_session_error)?;
    let mut snapshots = Vec::new();
    for row in rows {
        snapshots.push(row.map_err(map_sqlite_session_error)?);
    }
    drop(statement);

    let mut moved_scopes = Vec::new();
    for (scope_value, payload, updated_at) in snapshots {
        let scope = ReplayScope::from_string(scope_value.clone());
        let payload_value = serde_json::from_str::<serde_json::Value>(&payload).ok();
        let typed_snapshot = payload_value.as_ref().is_some_and(|value| {
            value.as_object().is_some_and(|object| {
                object.contains_key("revision")
                    || object.get("schema").and_then(serde_json::Value::as_str)
                        == Some(<ReplaySnapshot as starweaver_core::VersionedRecord>::SCHEMA)
            })
        });
        let mut snapshot = match from_versioned_json::<ReplaySnapshot>(&payload) {
            Ok(snapshot) => snapshot,
            Err(_) if !typed_snapshot => continue,
            Err(error) => return Err(SessionStoreError::Failed(error.to_string())),
        };
        if snapshot.scope.is_none() {
            snapshot.scope = Some(scope.clone());
        }
        if snapshot.scope.as_ref() != Some(&scope) {
            return Err(SessionStoreError::Failed(format!(
                "snapshot scope mismatch while splitting display scope {scope_value}"
            )));
        }
        if snapshot
            .cursor
            .as_ref()
            .is_some_and(|cursor| cursor.family == ReplayCursorFamily::ReplayEvent)
        {
            continue;
        }
        snapshot
            .validate(ReplayCursorFamily::Display, &scope)
            .map_err(|error| SessionStoreError::Failed(error.to_string()))?;
        let current_payload = to_versioned_json(&snapshot)
            .map_err(|error| SessionStoreError::Failed(error.to_string()))?;
        let inserted = connection
            .execute(
                "INSERT OR IGNORE INTO display_snapshot_records (scope, record, updated_at)
                 VALUES (?1, ?2, ?3)",
                params![scope_value, current_payload, updated_at],
            )
            .map_err(map_sqlite_session_error)?;
        if inserted == 0 {
            let persisted = connection
                .query_row(
                    "SELECT record FROM display_snapshot_records WHERE scope = ?1",
                    params![scope_value],
                    |row| row.get::<_, String>(0),
                )
                .map_err(map_sqlite_session_error)?;
            let persisted = from_versioned_json::<ReplaySnapshot>(&persisted)
                .map_err(|error| SessionStoreError::Failed(error.to_string()))?;
            if persisted != snapshot {
                return Err(SessionStoreError::Failed(format!(
                    "migration payload conflict while splitting display snapshot for scope {scope_value}"
                )));
            }
        }
        moved_scopes.push(scope_value);
    }
    for scope in moved_scopes {
        connection
            .execute(
                "DELETE FROM replay_snapshot_records WHERE scope = ?1",
                params![scope],
            )
            .map_err(map_sqlite_session_error)?;
    }
    Ok(())
}

fn backfill_namespaced_evidence_tables(connection: &Connection) -> SessionStoreResult<()> {
    backfill_compatible_table(
        connection,
        BackfillSpec {
            source_table: "sessions",
            target_table: "session_records",
            source_payload_columns: &["record_json"],
            target_columns: "session_id, record, created_at, updated_at",
            source_columns_prefix: "session_id",
            source_columns_suffix: ", created_at, updated_at",
            key_join: "target.session_id = source.session_id",
        },
    )?;
    backfill_compatible_table(
        connection,
        BackfillSpec {
            source_table: "runs",
            target_table: "run_records",
            source_payload_columns: &["record_json"],
            target_columns: "session_id, run_id, record, sequence_no, created_at, updated_at",
            source_columns_prefix: "session_id, run_id",
            source_columns_suffix: ", sequence_no, created_at, updated_at",
            key_join: "target.session_id = source.session_id AND target.run_id = source.run_id",
        },
    )?;
    backfill_compatible_table(
        connection,
        BackfillSpec {
            source_table: "raw_stream_records",
            target_table: "stream_records",
            source_payload_columns: &["record_json"],
            target_columns: "session_id, run_id, sequence_no, record",
            source_columns_prefix: "session_id, run_id, sequence_no",
            source_columns_suffix: "",
            key_join: "target.session_id = source.session_id AND target.run_id = source.run_id AND target.sequence_no = source.sequence_no",
        },
    )?;
    backfill_legacy_display_messages(connection)?;
    backfill_compatible_table(
        connection,
        BackfillSpec {
            source_table: "context_states",
            target_table: "run_context_records",
            source_payload_columns: &["state_json"],
            target_columns: "session_id, run_id, record, updated_at",
            source_columns_prefix: "session_id, run_id",
            source_columns_suffix: ", created_at",
            key_join: "target.session_id = source.session_id AND target.run_id = source.run_id",
        },
    )?;
    backfill_legacy_environment_states(connection)?;
    backfill_checkpoint_records(connection)?;
    backfill_compatible_table(
        connection,
        BackfillSpec {
            source_table: "approvals",
            target_table: "approval_records",
            source_payload_columns: &["record", "record_json"],
            target_columns: "session_id, run_id, approval_id, record, updated_at",
            source_columns_prefix: "session_id, run_id, approval_id",
            source_columns_suffix: ", updated_at",
            key_join: "target.session_id = source.session_id AND target.run_id = source.run_id AND target.approval_id = source.approval_id",
        },
    )?;
    backfill_compatible_table(
        connection,
        BackfillSpec {
            source_table: "deferred_tools",
            target_table: "deferred_tool_records",
            source_payload_columns: &["record", "record_json"],
            target_columns: "session_id, run_id, deferred_id, record, updated_at",
            source_columns_prefix: "session_id, run_id, deferred_id",
            source_columns_suffix: ", updated_at",
            key_join: "target.session_id = source.session_id AND target.run_id = source.run_id AND target.deferred_id = source.deferred_id",
        },
    )?;
    backfill_compatible_table(
        connection,
        BackfillSpec {
            source_table: "replay_snapshots",
            target_table: "replay_snapshot_records",
            source_payload_columns: &["record", "snapshot_json"],
            target_columns: "scope, record, updated_at",
            source_columns_prefix: "scope",
            source_columns_suffix: ", updated_at",
            key_join: "target.scope = source.scope",
        },
    )
}

fn backfill_legacy_display_messages(connection: &Connection) -> SessionStoreResult<()> {
    if !table_exists(connection, "display_messages")?
        || !table_has_column(connection, "display_messages", "message_json")?
    {
        return Ok(());
    }
    let mut statement = connection
        .prepare(
            "SELECT run_id, sequence_no, message_json, created_at
             FROM display_messages
             ORDER BY session_id, run_id, sequence_no",
        )
        .map_err(map_sqlite_session_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(map_sqlite_session_error)?;
    let mut records = Vec::new();
    for row in rows {
        records.push(row.map_err(map_sqlite_session_error)?);
    }
    drop(statement);
    for (run_id, sequence, message_payload, created_at) in records {
        let message = serde_json::from_str::<DisplayMessage>(&message_payload).map_err(|error| {
            SessionStoreError::Failed(format!(
                "invalid legacy display message for run {run_id} at sequence {sequence}: {error}"
            ))
        })?;
        let scope = ReplayScope::run(&run_id);
        let event = ReplayEvent::display(scope.clone(), message);
        let payload = to_versioned_json(&event)
            .map_err(|error| SessionStoreError::Failed(error.to_string()))?;
        let inserted = connection
            .execute(
                "INSERT OR IGNORE INTO replay_events (scope, sequence_no, record, created_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![scope.as_str(), sequence, payload, created_at],
            )
            .map_err(map_sqlite_session_error)?;
        if inserted == 0 {
            let persisted = connection
                .query_row(
                    "SELECT record FROM replay_events WHERE scope = ?1 AND sequence_no = ?2",
                    params![scope.as_str(), sequence],
                    |row| row.get::<_, String>(0),
                )
                .map_err(map_sqlite_session_error)?;
            let persisted = from_versioned_json::<ReplayEvent>(&persisted)
                .map_err(|error| SessionStoreError::Failed(error.to_string()))?;
            if persisted != event {
                return Err(SessionStoreError::Failed(format!(
                    "migration payload conflict while backfilling display message for scope {} at sequence {sequence}",
                    scope.as_str()
                )));
            }
        }
    }
    Ok(())
}

fn backfill_checkpoint_records(connection: &Connection) -> SessionStoreResult<()> {
    if !table_exists(connection, "checkpoints")? {
        return Ok(());
    }
    let payload_column = if table_has_column(connection, "checkpoints", "record")? {
        "record"
    } else if table_has_column(connection, "checkpoints", "checkpoint_json")? {
        "checkpoint_json"
    } else {
        return Err(SessionStoreError::Failed(
            "recognized checkpoints table has no supported payload column".to_string(),
        ));
    };
    let legacy_payloads = payload_column == "checkpoint_json";
    let mut statement = connection
        .prepare(&format!(
            "SELECT session_id, run_id, sequence_no, checkpoint_id, {payload_column}, created_at
             FROM checkpoints ORDER BY session_id, run_id, sequence_no, checkpoint_id"
        ))
        .map_err(map_sqlite_session_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })
        .map_err(map_sqlite_session_error)?;
    let mut rows_to_insert = Vec::new();
    for row in rows {
        let (session_id, run_id, sequence, checkpoint_id, payload, created_at) =
            row.map_err(map_sqlite_session_error)?;
        match from_versioned_json::<AgentCheckpoint>(&payload) {
            Ok(checkpoint) => {
                let typed_sequence = i64::try_from(checkpoint.run_step)
                    .map_err(|error| SessionStoreError::Failed(error.to_string()))?;
                if checkpoint.checkpoint_id.as_str() != checkpoint_id
                    || checkpoint.run_id.as_str() != run_id
                    || typed_sequence != sequence
                {
                    return Err(SessionStoreError::Failed(format!(
                        "legacy AgentCheckpoint identity mismatch for {checkpoint_id}"
                    )));
                }
                let current_payload = to_versioned_json(&checkpoint)
                    .map_err(|error| SessionStoreError::Failed(error.to_string()))?;
                rows_to_insert.push((
                    session_id,
                    run_id,
                    sequence,
                    checkpoint_id,
                    current_payload,
                    created_at,
                    checkpoint,
                ));
            }
            Err(agent_error) if legacy_payloads => {
                let reference = serde_json::from_str::<CheckpointRef>(&payload).map_err(|ref_error| {
                    SessionStoreError::Failed(format!(
                        "unsupported legacy checkpoint payload for {checkpoint_id}: AgentCheckpoint={agent_error}; CheckpointRef={ref_error}"
                    ))
                })?;
                let typed_sequence = i64::try_from(reference.sequence)
                    .map_err(|error| SessionStoreError::Failed(error.to_string()))?;
                if reference.checkpoint_id.as_str() != checkpoint_id
                    || reference.run_id.as_str() != run_id
                    || typed_sequence != sequence
                {
                    return Err(SessionStoreError::Failed(format!(
                        "legacy CheckpointRef identity mismatch for {checkpoint_id}"
                    )));
                }
            }
            Err(error) => {
                return Err(SessionStoreError::Failed(format!(
                    "invalid storage-v1 AgentCheckpoint payload for {checkpoint_id}: {error}"
                )));
            }
        }
    }
    drop(statement);

    for (session_id, run_id, sequence, checkpoint_id, payload, created_at, checkpoint) in
        rows_to_insert
    {
        let inserted = connection
            .execute(
                "INSERT OR IGNORE INTO checkpoint_records
                 (session_id, run_id, sequence_no, checkpoint_id, record, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    session_id,
                    run_id,
                    sequence,
                    checkpoint_id,
                    payload,
                    created_at
                ],
            )
            .map_err(map_sqlite_session_error)?;
        if inserted == 0 {
            let persisted = connection
                .query_row(
                    "SELECT record FROM checkpoint_records
                     WHERE session_id = ?1 AND run_id = ?2 AND checkpoint_id = ?3",
                    params![session_id, run_id, checkpoint_id],
                    |row| row.get::<_, String>(0),
                )
                .map_err(map_sqlite_session_error)?;
            let persisted =
                from_versioned_json::<AgentCheckpoint>(&persisted).map_err(|error| {
                    SessionStoreError::Failed(format!(
                        "invalid canonical AgentCheckpoint payload for {checkpoint_id}: {error}"
                    ))
                })?;
            if persisted != checkpoint {
                return Err(SessionStoreError::Failed(format!(
                    "migration payload conflict while backfilling checkpoint {checkpoint_id}"
                )));
            }
        }
    }
    Ok(())
}

fn backfill_legacy_environment_states(connection: &Connection) -> SessionStoreResult<()> {
    if !table_exists(connection, "environment_states")?
        || !table_has_column(connection, "environment_states", "state_json")?
    {
        return Ok(());
    }

    let source_conflicts: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM environment_states AS left_state
             JOIN environment_states AS right_state
               ON left_state.session_id = right_state.session_id
              AND left_state.run_id = right_state.run_id
              AND left_state.rowid < right_state.rowid
             WHERE left_state.state_json <> right_state.state_json",
            [],
            |row| row.get(0),
        )
        .map_err(map_sqlite_session_error)?;
    if source_conflicts > 0 {
        return Err(SessionStoreError::Failed(
            "migration payload conflict among legacy environment states".to_string(),
        ));
    }

    let target_conflicts: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM environment_states AS source
             JOIN run_environment_records AS target
               ON target.session_id = source.session_id
              AND target.run_id = source.run_id
             WHERE source.state_json <> target.record",
            [],
            |row| row.get(0),
        )
        .map_err(map_sqlite_session_error)?;
    if target_conflicts > 0 {
        return Err(SessionStoreError::Failed(
            "migration payload conflict while backfilling environment_states into run_environment_records"
                .to_string(),
        ));
    }

    connection
        .execute(
            "INSERT OR IGNORE INTO run_environment_records
             (session_id, run_id, record, updated_at)
             SELECT session_id, run_id, state_json, MAX(created_at)
             FROM environment_states
             GROUP BY session_id, run_id, state_json",
            [],
        )
        .map_err(map_sqlite_session_error)?;
    Ok(())
}

#[derive(Clone, Copy)]
struct BackfillSpec {
    source_table: &'static str,
    target_table: &'static str,
    source_payload_columns: &'static [&'static str],
    target_columns: &'static str,
    source_columns_prefix: &'static str,
    source_columns_suffix: &'static str,
    key_join: &'static str,
}

fn backfill_compatible_table(
    connection: &Connection,
    spec: BackfillSpec,
) -> SessionStoreResult<()> {
    if !table_exists(connection, spec.source_table)? {
        return Ok(());
    }
    let mut payload_column = None;
    for column in spec.source_payload_columns {
        if table_has_column(connection, spec.source_table, column)? {
            payload_column = Some(*column);
            break;
        }
    }
    let Some(payload_column) = payload_column else {
        return Ok(());
    };

    let conflict_count: i64 = connection
        .query_row(
            &format!(
                "SELECT COUNT(*) FROM {source} AS source
                 JOIN {target} AS target ON {key_join}
                 WHERE source.{payload} <> target.record",
                source = spec.source_table,
                target = spec.target_table,
                key_join = spec.key_join,
                payload = payload_column,
            ),
            [],
            |row| row.get(0),
        )
        .map_err(map_sqlite_session_error)?;
    if conflict_count > 0 {
        return Err(SessionStoreError::Failed(format!(
            "migration payload conflict while backfilling {} into {}",
            spec.source_table, spec.target_table
        )));
    }

    connection
        .execute(
            &format!(
                "INSERT OR IGNORE INTO {target} ({target_columns})
                 SELECT {source_prefix}, {payload}{source_suffix}
                 FROM {source}",
                target = spec.target_table,
                target_columns = spec.target_columns,
                source_prefix = spec.source_columns_prefix,
                payload = payload_column,
                source_suffix = spec.source_columns_suffix,
                source = spec.source_table,
            ),
            [],
        )
        .map_err(map_sqlite_session_error)?;
    Ok(())
}

fn applied_migration_checksums_valid(applied: &[SqliteAppliedMigration]) -> bool {
    applied.iter().all(|record| {
        SQLITE_MIGRATIONS
            .iter()
            .find(|migration| migration.id == record.id)
            .is_some_and(|migration| {
                record.checksum.as_deref() == Some(migration.checksum().as_str())
            })
    })
}

fn validate_and_backfill_migration_checksums(connection: &Connection) -> SessionStoreResult<()> {
    for record in load_applied_migration_records(connection)? {
        let migration = SQLITE_MIGRATIONS
            .iter()
            .find(|migration| migration.id == record.id)
            .ok_or_else(|| {
                SessionStoreError::Failed(format!(
                    "database contains unsupported future migration {}",
                    record.id
                ))
            })?;
        let expected = migration.checksum();
        match record.checksum {
            Some(actual) if actual != expected => {
                return Err(SessionStoreError::Failed(format!(
                    "migration checksum mismatch for {}: expected {expected}, found {actual}",
                    migration.id
                )));
            }
            Some(_) => {}
            None => {
                connection
                    .execute(
                        &format!(
                            "UPDATE {SQLITE_SCHEMA_MIGRATION_TABLE} SET checksum = ?1 WHERE id = ?2"
                        ),
                        params![expected, migration.id],
                    )
                    .map_err(map_sqlite_session_error)?;
            }
        }
    }
    Ok(())
}

fn ensure_migration_checksum_column(connection: &Connection) -> SessionStoreResult<()> {
    if !table_has_column(connection, SQLITE_SCHEMA_MIGRATION_TABLE, "checksum")? {
        connection
            .execute(
                &format!("ALTER TABLE {SQLITE_SCHEMA_MIGRATION_TABLE} ADD COLUMN checksum TEXT"),
                [],
            )
            .map_err(map_sqlite_session_error)?;
    }
    Ok(())
}

fn load_applied_migrations(connection: &Connection) -> SessionStoreResult<BTreeSet<String>> {
    let mut statement = connection
        .prepare(&format!("SELECT id FROM {SQLITE_SCHEMA_MIGRATION_TABLE}"))
        .map_err(map_sqlite_session_error)?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(map_sqlite_session_error)?;
    let mut applied = BTreeSet::new();
    for row in rows {
        applied.insert(row.map_err(map_sqlite_session_error)?);
    }
    Ok(applied)
}

fn table_exists(connection: &Connection, table: &str) -> SessionStoreResult<bool> {
    let count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            params![table],
            |row| row.get(0),
        )
        .map_err(map_sqlite_session_error)?;
    Ok(count > 0)
}

fn table_has_column(
    connection: &Connection,
    table: &str,
    column: &str,
) -> SessionStoreResult<bool> {
    if !table_exists(connection, table)? {
        return Ok(false);
    }
    let pragma = format!("PRAGMA table_info({table})");
    let mut statement = connection
        .prepare(&pragma)
        .map_err(map_sqlite_session_error)?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(map_sqlite_session_error)?;
    for row in rows {
        if row.map_err(map_sqlite_session_error)? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use rusqlite::{Connection, params};
    use starweaver_context::{AgentRunState, ResumableState};
    use starweaver_core::{AgentExecutionNode, ConversationId, RunId, SessionId};
    use starweaver_session::{RunRecord, SessionRecord, SessionStore};
    use starweaver_stream::{DisplayMessageKind, ReplayEventKind};

    use crate::SqliteSessionStore;

    use super::*;

    const LEGACY_TABLES: &str = r"
        CREATE TABLE sessions (
            session_id TEXT PRIMARY KEY,
            status TEXT NOT NULL,
            profile TEXT,
            title TEXT,
            head_run_id TEXT,
            head_success_run_id TEXT,
            active_run_id TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            record_json TEXT NOT NULL
        );
        CREATE TABLE runs (
            session_id TEXT NOT NULL,
            run_id TEXT NOT NULL,
            sequence_no INTEGER NOT NULL,
            status TEXT NOT NULL,
            restore_from_run_id TEXT,
            output_preview TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            record_json TEXT NOT NULL,
            PRIMARY KEY (session_id, run_id)
        );
        CREATE TABLE raw_stream_records (
            session_id TEXT NOT NULL,
            run_id TEXT NOT NULL,
            sequence_no INTEGER NOT NULL,
            kind TEXT NOT NULL,
            created_at TEXT NOT NULL,
            record_json TEXT NOT NULL,
            PRIMARY KEY (session_id, run_id, sequence_no)
        );
        CREATE TABLE display_messages (
            session_id TEXT NOT NULL,
            run_id TEXT NOT NULL,
            sequence_no INTEGER NOT NULL,
            kind TEXT NOT NULL,
            created_at TEXT NOT NULL,
            message_json TEXT NOT NULL,
            PRIMARY KEY (session_id, run_id, sequence_no)
        );
        CREATE TABLE context_states (
            session_id TEXT NOT NULL,
            run_id TEXT NOT NULL,
            state_json TEXT NOT NULL,
            created_at TEXT NOT NULL,
            PRIMARY KEY (session_id, run_id)
        );
        CREATE TABLE environment_states (
            ref_id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL,
            run_id TEXT NOT NULL,
            provider TEXT NOT NULL,
            state_json TEXT NOT NULL,
            created_at TEXT NOT NULL
        );
        CREATE TABLE checkpoints (
            checkpoint_id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL,
            run_id TEXT NOT NULL,
            sequence_no INTEGER NOT NULL,
            node TEXT NOT NULL,
            checkpoint_json TEXT NOT NULL,
            created_at TEXT NOT NULL
        );
        CREATE TABLE approvals (
            approval_id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL,
            run_id TEXT NOT NULL,
            action_id TEXT NOT NULL,
            action_name TEXT NOT NULL,
            status TEXT NOT NULL,
            record_json TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        CREATE TABLE deferred_tools (
            deferred_id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL,
            run_id TEXT NOT NULL,
            tool_call_id TEXT NOT NULL,
            tool_name TEXT NOT NULL,
            status TEXT NOT NULL,
            record_json TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        CREATE TABLE replay_snapshots (
            scope TEXT PRIMARY KEY,
            snapshot_json TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
    ";

    fn mark_migration_applied(connection: &Connection, index: usize) {
        let migration = &SQLITE_MIGRATIONS[index];
        connection
            .execute_batch(&format!(
                "CREATE TABLE IF NOT EXISTS {SQLITE_SCHEMA_MIGRATION_TABLE} (
                    id TEXT PRIMARY KEY,
                    description TEXT NOT NULL,
                    checksum TEXT,
                    applied_at TEXT NOT NULL
                );"
            ))
            .expect("create migration table");
        connection
            .execute(
                &format!(
                    "INSERT INTO {SQLITE_SCHEMA_MIGRATION_TABLE}
                     (id, description, checksum, applied_at) VALUES (?1, ?2, ?3, ?4)"
                ),
                params![
                    migration.id,
                    migration.description,
                    migration.checksum(),
                    "2026-07-11T00:00:00Z"
                ],
            )
            .expect("mark migration applied");
    }

    fn mark_storage_v1_applied(connection: &Connection) {
        mark_migration_applied(connection, 0);
    }

    fn scalar_string(connection: &Connection, sql: &str) -> String {
        connection
            .query_row(sql, [], |row| row.get(0))
            .expect("read scalar string")
    }

    fn scalar_i64(connection: &Connection, sql: &str) -> i64 {
        connection
            .query_row(sql, [], |row| row.get(0))
            .expect("read scalar integer")
    }

    fn test_checkpoint(run_id: &str, step: usize) -> AgentCheckpoint {
        let mut state = AgentRunState::new(RunId::from_string(run_id), ConversationId::new());
        state.run_step = step;
        AgentCheckpoint::new(AgentExecutionNode::RunStart, &state)
    }

    fn checkpoint_reference(checkpoint: &AgentCheckpoint) -> CheckpointRef {
        CheckpointRef {
            checkpoint_id: checkpoint.checkpoint_id.clone(),
            run_id: checkpoint.run_id.clone(),
            sequence: checkpoint.run_step,
            node: format!("{:?}", checkpoint.node),
            storage_ref: None,
            stream_cursor: checkpoint.resume.cursor.stream_cursor,
            created_at: Utc::now(),
            metadata: checkpoint.metadata.clone(),
        }
    }

    #[test]
    fn legacy_cli_tables_are_backfilled_without_mutating_legacy_schema() {
        let mut connection = Connection::open_in_memory().expect("open database");
        connection
            .execute_batch(LEGACY_TABLES)
            .expect("legacy schema");
        connection
            .execute_batch(
                r#"
                INSERT INTO sessions VALUES
                    ('session-1', 'active', NULL, NULL, 'run-1', NULL, 'run-1', '2026-07-11T00:00:00Z', '2026-07-11T00:00:01Z', '{"kind":"legacy-session"}');
                INSERT INTO runs VALUES
                    ('session-1', 'run-1', 1, 'running', NULL, NULL, '2026-07-11T00:00:00Z', '2026-07-11T00:00:01Z', '{"kind":"legacy-run"}');
                INSERT INTO raw_stream_records VALUES
                    ('session-1', 'run-1', 7, 'model_delta', '2026-07-11T00:00:02Z', '{"kind":"legacy-stream"}');
                INSERT INTO context_states VALUES
                    ('session-1', 'run-1', '{"kind":"legacy-context"}', '2026-07-11T00:00:03Z');
                INSERT INTO environment_states VALUES
                    ('environment-1', 'session-1', 'run-1', 'local', '{"kind":"legacy-environment"}', '2026-07-11T00:00:04Z'),
                    ('environment-2', 'session-1', 'run-1', 'envd', '{"kind":"legacy-environment"}', '2026-07-11T00:00:05Z');
                INSERT INTO approvals VALUES
                    ('approval-1', 'session-1', 'run-1', 'action-1', 'shell', 'pending', '{"kind":"legacy-approval"}', '2026-07-11T00:00:00Z', '2026-07-11T00:00:01Z');
                INSERT INTO deferred_tools VALUES
                    ('deferred-1', 'session-1', 'run-1', 'call-1', 'shell', 'pending', '{"kind":"legacy-deferred"}', '2026-07-11T00:00:00Z', '2026-07-11T00:00:02Z');
                INSERT INTO replay_snapshots VALUES
                    ('session:session-1', '{"kind":"legacy-snapshot"}', '2026-07-11T00:00:03Z');
                "#,
            )
            .expect("legacy records");
        let display = DisplayMessage::new(
            2,
            SessionId::from_string("session-1"),
            RunId::from_string("run-1"),
            DisplayMessageKind::HostEvent,
        )
        .with_preview("legacy display");
        let mut legacy_display = serde_json::to_value(&display).expect("serialize display");
        legacy_display["type"] = serde_json::json!("HOST_OPERATION");
        connection
            .execute(
                "INSERT INTO display_messages
                 (session_id, run_id, sequence_no, kind, created_at, message_json)
                 VALUES ('session-1', 'run-1', 2, 'host_operation', ?1, ?2)",
                params![
                    display.timestamp.to_rfc3339(),
                    serde_json::to_string(&legacy_display).expect("serialize legacy display")
                ],
            )
            .expect("legacy display message");
        let reference_source = test_checkpoint("run-1", 3);
        let reference = checkpoint_reference(&reference_source);
        let full_checkpoint = test_checkpoint("run-1", 4);
        connection
            .execute(
                "INSERT INTO checkpoints
                 (checkpoint_id, session_id, run_id, sequence_no, node, checkpoint_json, created_at)
                 VALUES (?1, 'session-1', 'run-1', ?2, ?3, ?4, '2026-07-11T00:00:00Z')",
                params![
                    reference.checkpoint_id.as_str(),
                    i64::try_from(reference.sequence).expect("reference sequence"),
                    reference.node,
                    serde_json::to_string(&reference).expect("serialize reference")
                ],
            )
            .expect("legacy checkpoint reference");
        connection
            .execute(
                "INSERT INTO checkpoints
                 (checkpoint_id, session_id, run_id, sequence_no, node, checkpoint_json, created_at)
                 VALUES (?1, 'session-1', 'run-1', ?2, ?3, ?4, '2026-07-11T00:00:01Z')",
                params![
                    full_checkpoint.checkpoint_id.as_str(),
                    i64::try_from(full_checkpoint.run_step).expect("checkpoint sequence"),
                    format!("{:?}", full_checkpoint.node),
                    serde_json::to_string(&full_checkpoint).expect("serialize checkpoint")
                ],
            )
            .expect("legacy full checkpoint");

        let applied = apply_sqlite_migrations(&mut connection).expect("migrate legacy database");
        assert_eq!(applied.len(), 7);
        assert_eq!(
            scalar_string(&connection, "SELECT record FROM session_records"),
            r#"{"kind":"legacy-session"}"#
        );
        assert_eq!(
            scalar_string(&connection, "SELECT record FROM run_records"),
            r#"{"kind":"legacy-run"}"#
        );
        assert_eq!(
            scalar_string(&connection, "SELECT record FROM stream_records"),
            r#"{"kind":"legacy-stream"}"#
        );
        let replay = from_versioned_json::<ReplayEvent>(&scalar_string(
            &connection,
            "SELECT record FROM display_message_records WHERE scope = 'run:run-1' AND sequence_no = 2",
        ))
        .expect("decode migrated display archive");
        let ReplayEventKind::DisplayMessage(migrated_display) = replay.event else {
            panic!("expected migrated display message");
        };
        assert_eq!(migrated_display.preview.as_deref(), Some("legacy display"));
        assert_eq!(migrated_display.kind, DisplayMessageKind::HostEvent);
        assert_eq!(
            scalar_i64(
                &connection,
                "SELECT COUNT(*) FROM replay_events WHERE scope = 'run:run-1'"
            ),
            0,
            "legacy display rows move out of the replay-event family"
        );
        assert_eq!(
            scalar_string(&connection, "SELECT record FROM run_context_records"),
            r#"{"kind":"legacy-context"}"#
        );
        assert_eq!(
            scalar_string(&connection, "SELECT record FROM run_environment_records"),
            r#"{"kind":"legacy-environment"}"#
        );
        assert_eq!(
            scalar_i64(&connection, "SELECT COUNT(*) FROM run_environment_records"),
            1
        );
        assert_eq!(
            scalar_i64(&connection, "SELECT COUNT(*) FROM checkpoint_records"),
            1,
            "full AgentCheckpoint rows migrate while CheckpointRef rows remain legacy-only"
        );
        let migrated_checkpoint = from_versioned_json::<AgentCheckpoint>(&scalar_string(
            &connection,
            "SELECT record FROM checkpoint_records",
        ))
        .expect("typed migrated checkpoint");
        assert_eq!(migrated_checkpoint, full_checkpoint);
        assert_eq!(
            scalar_string(
                &connection,
                &format!(
                    "SELECT checkpoint_json FROM checkpoints WHERE checkpoint_id = '{}'",
                    reference.checkpoint_id.as_str()
                )
            ),
            serde_json::to_string(&reference).expect("serialize reference")
        );
        assert_eq!(
            scalar_string(&connection, "SELECT record FROM approval_records"),
            r#"{"kind":"legacy-approval"}"#
        );
        assert_eq!(
            scalar_string(&connection, "SELECT record FROM deferred_tool_records"),
            r#"{"kind":"legacy-deferred"}"#
        );
        assert_eq!(
            scalar_string(&connection, "SELECT record FROM replay_snapshot_records"),
            r#"{"kind":"legacy-snapshot"}"#
        );
        assert!(
            table_has_column(&connection, "checkpoints", "checkpoint_json")
                .expect("inspect legacy checkpoint payload column")
        );
        assert!(
            !table_has_column(&connection, "checkpoints", "record")
                .expect("inspect storage-v1 checkpoint payload column")
        );

        let second = apply_sqlite_migrations(&mut connection).expect("repeat migration");
        assert!(second.is_empty());
        assert_eq!(
            scalar_i64(&connection, "SELECT COUNT(*) FROM checkpoint_records"),
            1
        );
    }

    #[tokio::test]
    async fn typed_legacy_session_run_and_context_are_readable_after_migration() {
        let mut connection = Connection::open_in_memory().expect("open database");
        connection
            .execute_batch(LEGACY_TABLES)
            .expect("legacy schema");
        let session_id = SessionId::from_string("session-typed");
        let run_id = RunId::from_string("run-typed");
        let mut session = SessionRecord::new(session_id.clone());
        session.state = ResumableState {
            started_at: Utc::now(),
            notes: std::collections::BTreeMap::from([(
                "source".to_string(),
                "session-head".to_string(),
            )]),
            ..ResumableState::default()
        };
        session.head_run_id = Some(run_id.clone());
        let mut run = RunRecord::new(session_id.clone(), run_id.clone(), ConversationId::new());
        run.sequence_no = 1;
        let run_state = ResumableState {
            started_at: Utc::now(),
            notes: std::collections::BTreeMap::from([(
                "source".to_string(),
                "selected-run".to_string(),
            )]),
            ..ResumableState::default()
        };
        connection
            .execute(
                "INSERT INTO sessions VALUES
                 (?1, 'active', NULL, NULL, ?2, NULL, NULL, ?3, ?4, ?5)",
                params![
                    session_id.as_str(),
                    run_id.as_str(),
                    session.created_at.to_rfc3339(),
                    session.updated_at.to_rfc3339(),
                    serde_json::to_string(&session).expect("serialize session")
                ],
            )
            .expect("legacy session");
        connection
            .execute(
                "INSERT INTO runs VALUES
                 (?1, ?2, 1, 'queued', NULL, NULL, ?3, ?4, ?5)",
                params![
                    session_id.as_str(),
                    run_id.as_str(),
                    run.created_at.to_rfc3339(),
                    run.updated_at.to_rfc3339(),
                    serde_json::to_string(&run).expect("serialize run")
                ],
            )
            .expect("legacy run");
        connection
            .execute(
                "INSERT INTO context_states VALUES (?1, ?2, ?3, ?4)",
                params![
                    session_id.as_str(),
                    run_id.as_str(),
                    serde_json::to_string(&run_state).expect("serialize state"),
                    Utc::now().to_rfc3339()
                ],
            )
            .expect("legacy context state");

        apply_sqlite_migrations(&mut connection).expect("migrate typed legacy database");
        let store = SqliteSessionStore::from_shared(Arc::new(Mutex::new(connection)));
        assert_eq!(
            store
                .load_session(&session_id)
                .await
                .expect("load migrated session"),
            session
        );
        assert_eq!(
            store
                .load_run(&session_id, &run_id)
                .await
                .expect("load migrated run"),
            run
        );
        let snapshot = store
            .resume_snapshot(&session_id, &run_id)
            .await
            .expect("resume migrated run");
        assert_eq!(snapshot.state, run_state);
    }

    #[test]
    fn storage_v1_tables_are_backfilled_into_namespaced_tables() {
        let mut connection = Connection::open_in_memory().expect("open database");
        connection
            .execute_batch(SQLITE_MIGRATIONS[0].sql)
            .expect("storage v1 schema");
        mark_storage_v1_applied(&connection);
        let session = SessionRecord::new(SessionId::from_string("session-v1"));
        let mut run = RunRecord::new(
            session.session_id.clone(),
            RunId::from_string("run-v1"),
            ConversationId::new(),
        );
        run.sequence_no = 1;
        run.metadata.insert(
            "starweaver.run_evidence_sha256".to_string(),
            serde_json::json!("caller-forged-digest"),
        );
        connection
            .execute(
                "INSERT INTO session_records VALUES (?1, ?2, ?3, ?4)",
                params![
                    session.session_id.as_str(),
                    serde_json::to_string(&session).expect("serialize session"),
                    session.created_at.to_rfc3339(),
                    session.updated_at.to_rfc3339()
                ],
            )
            .expect("storage v1 session");
        connection
            .execute(
                "INSERT INTO run_records VALUES (?1, ?2, ?3, 1, ?4, ?5)",
                params![
                    run.session_id.as_str(),
                    run.run_id.as_str(),
                    serde_json::to_string(&run).expect("serialize run"),
                    run.created_at.to_rfc3339(),
                    run.updated_at.to_rfc3339()
                ],
            )
            .expect("storage v1 run");
        connection
            .execute_batch(
                r#"
                INSERT INTO approvals VALUES
                    ('session-v1', 'run-v1', 'approval-v1', '{"kind":"v1-approval"}', '2026-07-11T00:00:01Z');
                INSERT INTO deferred_tools VALUES
                    ('session-v1', 'run-v1', 'deferred-v1', '{"kind":"v1-deferred"}', '2026-07-11T00:00:02Z');
                INSERT INTO replay_snapshots VALUES
                    ('run:run-v1', '{"kind":"v1-snapshot"}', '2026-07-11T00:00:03Z');
                "#,
            )
            .expect("storage v1 records");
        let checkpoint = test_checkpoint("run-v1", 4);
        connection
            .execute(
                "INSERT INTO checkpoints VALUES
                 ('session-v1', 'run-v1', ?1, ?2, ?3, '2026-07-11T00:00:00Z')",
                params![
                    i64::try_from(checkpoint.run_step).expect("checkpoint sequence"),
                    checkpoint.checkpoint_id.as_str(),
                    serde_json::to_string(&checkpoint).expect("serialize checkpoint")
                ],
            )
            .expect("storage v1 checkpoint");

        let applied = apply_sqlite_migrations(&mut connection).expect("migrate storage v1");
        assert_eq!(
            applied,
            vec![
                "20260711_000002_namespaced_evidence_tables",
                "20260711_000003_split_display_and_replay_families",
                "20260712_000004_evidence_outbox_and_resume_claims",
                "20260714_000005_agent_session_management",
                "20260714_000006_async_subagent_delivery",
                "20260715_000007_background_terminal_fingerprint",
            ]
        );
        let migrated_checkpoint = from_versioned_json::<AgentCheckpoint>(&scalar_string(
            &connection,
            "SELECT record FROM checkpoint_records",
        ))
        .expect("typed storage-v1 checkpoint");
        assert_eq!(migrated_checkpoint, checkpoint);
        assert_eq!(
            scalar_string(&connection, "SELECT record FROM approval_records"),
            r#"{"kind":"v1-approval"}"#
        );
        assert_eq!(
            scalar_string(&connection, "SELECT record FROM deferred_tool_records"),
            r#"{"kind":"v1-deferred"}"#
        );
        assert_eq!(
            scalar_string(&connection, "SELECT record FROM replay_snapshot_records"),
            r#"{"kind":"v1-snapshot"}"#
        );
        assert_eq!(
            scalar_string(&connection, "SELECT digest FROM run_evidence_commits"),
            LEGACY_UNSEALED_EVIDENCE_DIGEST
        );
    }

    #[test]
    fn mixed_legacy_and_storage_v1_tables_are_detected_per_table() {
        let mut connection = Connection::open_in_memory().expect("open database");
        connection
            .execute_batch(
                r"
                CREATE TABLE run_records (
                    session_id TEXT NOT NULL, run_id TEXT NOT NULL, record TEXT NOT NULL,
                    sequence_no INTEGER NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
                    PRIMARY KEY (session_id, run_id)
                );
                CREATE TABLE checkpoints (
                    session_id TEXT NOT NULL, run_id TEXT NOT NULL, sequence_no INTEGER NOT NULL,
                    checkpoint_id TEXT NOT NULL, record TEXT NOT NULL, created_at TEXT NOT NULL,
                    PRIMARY KEY (session_id, run_id, sequence_no, checkpoint_id)
                );
                CREATE TABLE approvals (
                    approval_id TEXT PRIMARY KEY, session_id TEXT NOT NULL, run_id TEXT NOT NULL,
                    action_id TEXT NOT NULL, action_name TEXT NOT NULL, status TEXT NOT NULL,
                    record_json TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL
                );
                CREATE TABLE deferred_tools (
                    session_id TEXT NOT NULL, run_id TEXT NOT NULL, deferred_id TEXT NOT NULL,
                    record TEXT NOT NULL, updated_at TEXT NOT NULL,
                    PRIMARY KEY (session_id, run_id, deferred_id)
                );
                CREATE TABLE replay_snapshots (
                    scope TEXT PRIMARY KEY, snapshot_json TEXT NOT NULL, updated_at TEXT NOT NULL
                );
                INSERT INTO approvals VALUES
                    ('approval-mixed', 'session-mixed', 'run-mixed', 'action', 'tool', 'pending', 'approval-legacy', 'time-1', 'time-2');
                INSERT INTO deferred_tools VALUES
                    ('session-mixed', 'run-mixed', 'deferred-mixed', 'deferred-v1', 'time-3');
                INSERT INTO replay_snapshots VALUES
                    ('mixed', 'snapshot-legacy', 'time-4');
                ",
            )
            .expect("mixed schema and records");
        let checkpoint = test_checkpoint("run-mixed", 1);
        connection
            .execute(
                "INSERT INTO run_records VALUES
                 ('session-mixed', 'run-mixed', 'run-record', 1, 'time-1', 'time-1')",
                [],
            )
            .expect("mixed parent run");
        connection
            .execute(
                "INSERT INTO checkpoints VALUES
                 ('session-mixed', 'run-mixed', ?1, ?2, ?3, 'time-1')",
                params![
                    i64::try_from(checkpoint.run_step).expect("checkpoint sequence"),
                    checkpoint.checkpoint_id.as_str(),
                    serde_json::to_string(&checkpoint).expect("serialize checkpoint")
                ],
            )
            .expect("mixed checkpoint");
        mark_storage_v1_applied(&connection);

        apply_sqlite_migrations(&mut connection).expect("migrate mixed database");
        let migrated = from_versioned_json::<AgentCheckpoint>(&scalar_string(
            &connection,
            "SELECT record FROM checkpoint_records",
        ))
        .expect("typed mixed checkpoint");
        assert_eq!(migrated, checkpoint);
        assert_eq!(
            scalar_string(&connection, "SELECT record FROM approval_records"),
            "approval-legacy"
        );
        assert_eq!(
            scalar_string(&connection, "SELECT record FROM deferred_tool_records"),
            "deferred-v1"
        );
        assert_eq!(
            scalar_string(&connection, "SELECT record FROM replay_snapshot_records"),
            "snapshot-legacy"
        );
    }

    #[test]
    fn typed_legacy_display_snapshot_moves_to_its_family_table() {
        let mut connection = Connection::open_in_memory().expect("open database");
        connection
            .execute_batch(SQLITE_MIGRATIONS[0].sql)
            .expect("storage v1 schema");
        connection
            .execute_batch(SQLITE_MIGRATIONS[1].sql)
            .expect("namespaced schema");
        mark_migration_applied(&connection, 0);
        mark_migration_applied(&connection, 1);

        let display_scope = ReplayScope::run("run-display-snapshot");
        let event_scope = ReplayScope::run("run-event-snapshot");
        let legacy_display = serde_json::json!({
            "scope": display_scope.as_str(),
            "revision": 4,
            "cursor": {
                "scope": display_scope.as_str(),
                "sequence": 8
            },
            "display_messages": []
        });
        let event_snapshot = ReplaySnapshot {
            scope: Some(event_scope.clone()),
            revision: 5,
            cursor: Some(starweaver_stream::ReplayCursor::replay_event(
                event_scope.clone(),
                9,
            )),
            display_messages: Vec::new(),
            metadata: serde_json::Map::default(),
        };
        connection
            .execute(
                "INSERT INTO replay_snapshot_records VALUES (?1, ?2, 'time-1')",
                params![display_scope.as_str(), legacy_display.to_string()],
            )
            .expect("legacy display snapshot");
        connection
            .execute(
                "INSERT INTO replay_snapshot_records VALUES (?1, ?2, 'time-2')",
                params![
                    event_scope.as_str(),
                    to_versioned_json(&event_snapshot).expect("serialize event snapshot")
                ],
            )
            .expect("typed event snapshot");

        let applied = apply_sqlite_migrations(&mut connection).expect("split snapshot families");
        assert_eq!(
            applied,
            vec![
                "20260711_000003_split_display_and_replay_families",
                "20260712_000004_evidence_outbox_and_resume_claims",
                "20260714_000005_agent_session_management",
                "20260714_000006_async_subagent_delivery",
                "20260715_000007_background_terminal_fingerprint",
            ]
        );
        let moved = from_versioned_json::<ReplaySnapshot>(&scalar_string(
            &connection,
            "SELECT record FROM display_snapshot_records WHERE scope = 'run:run-display-snapshot'",
        ))
        .expect("decode moved display snapshot");
        assert_eq!(moved.scope.as_ref(), Some(&display_scope));
        assert_eq!(
            moved.cursor.as_ref().map(|cursor| cursor.family),
            Some(ReplayCursorFamily::Display)
        );
        assert_eq!(
            scalar_i64(
                &connection,
                "SELECT COUNT(*) FROM replay_snapshot_records WHERE scope = 'run:run-display-snapshot'"
            ),
            0
        );
        assert_eq!(
            from_versioned_json::<ReplaySnapshot>(&scalar_string(
                &connection,
                "SELECT record FROM replay_snapshot_records WHERE scope = 'run:run-event-snapshot'",
            ))
            .expect("decode retained event snapshot"),
            event_snapshot
        );
    }

    #[test]
    fn split_display_migration_canonicalizes_legacy_host_operation_event_names() {
        let mut connection = Connection::open_in_memory().expect("open database");
        connection
            .execute_batch(SQLITE_MIGRATIONS[0].sql)
            .expect("storage v1 schema");
        connection
            .execute_batch(SQLITE_MIGRATIONS[1].sql)
            .expect("namespaced schema");
        connection
            .execute_batch(SQLITE_MIGRATIONS[2].sql)
            .expect("pre-existing display tables");
        mark_migration_applied(&connection, 0);
        mark_migration_applied(&connection, 1);

        let scope = ReplayScope::run("run-legacy-host-operation");
        let event = ReplayEvent::display(
            scope.clone(),
            DisplayMessage::new(
                1,
                SessionId::from_string("session-legacy-host-operation"),
                RunId::from_string("run-legacy-host-operation"),
                DisplayMessageKind::HostEvent,
            ),
        );
        let legacy_payload = to_versioned_json(&event)
            .expect("serialize display event")
            .replace("HOST_EVENT", "HOST_OPERATION");
        connection
            .execute(
                "INSERT INTO replay_events VALUES (?1, ?2, ?3, 'time-source')",
                params![scope.as_str(), 1_i64, legacy_payload],
            )
            .expect("legacy display event");

        apply_sqlite_migrations(&mut connection).expect("split legacy display event");

        let payload = scalar_string(
            &connection,
            "SELECT record FROM display_message_records
             WHERE scope = 'run:run-legacy-host-operation' AND sequence_no = 1",
        );
        assert!(payload.contains("HOST_EVENT"));
        assert!(!payload.contains("HOST_OPERATION"));
        let replay = from_versioned_json::<ReplayEvent>(&payload).expect("decode canonical replay");
        let ReplayEventKind::DisplayMessage(message) = replay.event else {
            panic!("expected migrated display message");
        };
        assert_eq!(message.kind, DisplayMessageKind::HostEvent);
        assert_eq!(
            scalar_i64(&connection, "SELECT COUNT(*) FROM replay_events"),
            0,
            "source display rows move out of the replay-event family"
        );
    }

    #[test]
    fn display_family_conflict_rolls_back_the_entire_split_migration() {
        let mut connection = Connection::open_in_memory().expect("open database");
        connection
            .execute_batch(SQLITE_MIGRATIONS[0].sql)
            .expect("storage v1 schema");
        connection
            .execute_batch(SQLITE_MIGRATIONS[1].sql)
            .expect("namespaced schema");
        connection
            .execute_batch(SQLITE_MIGRATIONS[2].sql)
            .expect("pre-existing display tables");
        mark_migration_applied(&connection, 0);
        mark_migration_applied(&connection, 1);

        let scope = ReplayScope::run("run-display-conflict");
        let first_message = DisplayMessage::new(
            1,
            SessionId::from_string("session-display-conflict"),
            RunId::from_string("run-display-conflict"),
            DisplayMessageKind::AssistantTextDelta,
        )
        .with_preview("must roll back");
        let conflicting_source_message = DisplayMessage::new(
            2,
            SessionId::from_string("session-display-conflict"),
            RunId::from_string("run-display-conflict"),
            DisplayMessageKind::AssistantTextDelta,
        )
        .with_preview("source");
        let conflicting_target_message = DisplayMessage::new(
            2,
            SessionId::from_string("session-display-conflict"),
            RunId::from_string("run-display-conflict"),
            DisplayMessageKind::AssistantTextDelta,
        )
        .with_preview("target");
        let first_event = ReplayEvent::display(scope.clone(), first_message);
        let source_event = ReplayEvent::display(scope.clone(), conflicting_source_message);
        let target_event = ReplayEvent::display(scope.clone(), conflicting_target_message);
        for event in [&first_event, &source_event] {
            connection
                .execute(
                    "INSERT INTO replay_events VALUES (?1, ?2, ?3, 'time-source')",
                    params![
                        scope.as_str(),
                        i64::try_from(event.sequence).expect("event sequence"),
                        to_versioned_json(event).expect("serialize source event")
                    ],
                )
                .expect("source display event");
        }
        connection
            .execute(
                "INSERT INTO display_message_records VALUES (?1, 2, ?2, 'time-target')",
                params![
                    scope.as_str(),
                    to_versioned_json(&target_event).expect("serialize target event")
                ],
            )
            .expect("conflicting display target");

        let error = apply_sqlite_migrations(&mut connection).expect_err("reject display conflict");
        assert!(error.to_string().contains("migration payload conflict"));
        assert_eq!(
            scalar_i64(&connection, "SELECT COUNT(*) FROM replay_events"),
            2,
            "source rows remain because the split transaction rolled back"
        );
        assert_eq!(
            scalar_i64(&connection, "SELECT COUNT(*) FROM display_message_records"),
            1,
            "the earlier non-conflicting insert rolled back"
        );
        assert_eq!(
            scalar_i64(
                &connection,
                &format!(
                    "SELECT COUNT(*) FROM {SQLITE_SCHEMA_MIGRATION_TABLE} WHERE id = '20260711_000003_split_display_and_replay_families'"
                )
            ),
            0
        );
    }

    #[test]
    fn malformed_legacy_checkpoint_payload_rejects_migration() {
        let mut connection = Connection::open_in_memory().expect("open database");
        connection
            .execute_batch(
                r#"
                CREATE TABLE checkpoints (
                    checkpoint_id TEXT PRIMARY KEY,
                    session_id TEXT NOT NULL,
                    run_id TEXT NOT NULL,
                    sequence_no INTEGER NOT NULL,
                    node TEXT NOT NULL,
                    checkpoint_json TEXT NOT NULL,
                    created_at TEXT NOT NULL
                );
                INSERT INTO checkpoints VALUES
                    ('checkpoint-bad', 'session-bad', 'run-bad', 1, 'RunStart',
                     '{"not":"a supported checkpoint"}', 'time-1');
                "#,
            )
            .expect("legacy malformed checkpoint");

        let error = apply_sqlite_migrations(&mut connection).expect_err("reject malformed payload");
        assert!(
            error
                .to_string()
                .contains("unsupported legacy checkpoint payload")
        );
        assert!(
            !table_exists(&connection, "checkpoint_records")
                .expect("inspect rolled-back canonical table")
        );
    }

    #[test]
    fn payload_conflict_rolls_back_the_entire_namespaced_migration() {
        let mut connection = Connection::open_in_memory().expect("open database");
        connection
            .execute_batch(SQLITE_MIGRATIONS[0].sql)
            .expect("storage v1 schema");
        mark_storage_v1_applied(&connection);
        connection
            .execute_batch(
                r"
                INSERT INTO approvals VALUES
                    ('session-conflict', 'run-conflict', 'approval-conflict', 'source-payload', 'time-1'),
                    ('session-conflict', 'run-conflict', 'approval-not-copied', 'other-source', 'time-2');
                CREATE TABLE approval_records (
                    session_id TEXT NOT NULL, run_id TEXT NOT NULL, approval_id TEXT NOT NULL,
                    record TEXT NOT NULL, updated_at TEXT NOT NULL,
                    PRIMARY KEY (session_id, run_id, approval_id)
                );
                INSERT INTO approval_records VALUES
                    ('session-conflict', 'run-conflict', 'approval-conflict', 'different-payload', 'time-1');
                ",
            )
            .expect("conflicting fixtures");

        let error = apply_sqlite_migrations(&mut connection).expect_err("reject conflict");
        assert!(error.to_string().contains("migration payload conflict"));
        assert_eq!(
            scalar_i64(&connection, "SELECT COUNT(*) FROM approval_records"),
            1
        );
        assert!(
            !table_exists(&connection, "run_context_records")
                .expect("inspect rolled-back context table")
        );
        assert_eq!(
            scalar_i64(
                &connection,
                &format!(
                    "SELECT COUNT(*) FROM {SQLITE_SCHEMA_MIGRATION_TABLE} WHERE id = '20260711_000002_namespaced_evidence_tables'"
                )
            ),
            0
        );
    }

    #[test]
    fn future_migration_is_reported_and_rejected_fail_closed() {
        let mut connection = Connection::open_in_memory().expect("open database");
        apply_sqlite_migrations(&mut connection).expect("apply current migrations");
        connection
            .execute(
                &format!(
                    "INSERT INTO {SQLITE_SCHEMA_MIGRATION_TABLE} (id, description, checksum, applied_at) VALUES (?1, ?2, ?3, ?4)"
                ),
                params![
                    "99999999_999999_future",
                    "future migration",
                    "future-checksum",
                    Utc::now().to_rfc3339()
                ],
            )
            .expect("insert future migration");

        let status = sqlite_migration_status_for_connection(&connection).expect("migration status");
        assert!(!status.current);
        assert!(!status.checksums_valid());
        let error = apply_sqlite_migrations(&mut connection).expect_err("reject future migration");
        assert!(error.to_string().contains("unsupported future migration"));
    }
}
