//! Product-neutral local database location and legacy project-store import.

use std::{fs, path::Path, path::PathBuf};

use chrono::Utc;
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde_json::Value;
use starweaver_session::{SessionRecord, SessionStoreError, SessionStoreResult};

use crate::{
    SqliteStorage, migrate_sqlite_database,
    session_store::records::save_session_record,
    sqlite::{deserialize_json_record, map_sqlite_session_error},
};

/// Canonical filename for the machine-local durable session database.
pub const CANONICAL_SESSION_DATABASE_FILENAME: &str = "starweaver.sqlite";

/// Metadata key identifying the product that originally created a session.
pub const SESSION_SOURCE_PRODUCT_METADATA_KEY: &str = "starweaver.source_product";

/// Metadata key recording the legacy database from which a session was imported.
pub const SESSION_IMPORTED_FROM_METADATA_KEY: &str = "starweaver.imported_from";

/// Resolve the canonical session database below a Starweaver config directory.
#[must_use]
pub fn canonical_session_database_path(config_dir: impl AsRef<Path>) -> PathBuf {
    config_dir
        .as_ref()
        .join(CANONICAL_SESSION_DATABASE_FILENAME)
}

/// Result of attempting an idempotent project-local database import.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalStoreImportReport {
    /// Normalized source database path.
    pub source_path: PathBuf,
    /// Normalized workspace display value applied to legacy sessions that did not have one.
    pub workspace: String,
    /// Number of newly imported sessions.
    pub sessions_imported: usize,
    /// Number of durable rows inserted, including sessions.
    pub rows_imported: usize,
    /// True when this call inserted at least one new destination row.
    pub imported: bool,
}

impl SqliteStorage {
    /// Import missing sessions and their durable evidence from a legacy project-local database.
    ///
    /// Existing destination sessions win. Sessions first imported from this source are tracked
    /// individually so later calls can copy newly added evidence without allowing a colliding,
    /// independently-created destination session to inherit source evidence. Admission leases,
    /// mutation/control receipts, background execution ownership, migration bookkeeping, and
    /// publication outbox state are deliberately not copied because they are process-control state
    /// rather than portable session evidence. The source is upgraded to the current schema before
    /// each import. Mutable records owned by a tracked source session are synchronized with
    /// conflict-aware upserts; append-only evidence uses conflict-ignore inserts.
    ///
    /// # Errors
    ///
    /// Returns a store error when paths cannot be normalized, the source cannot be migrated, or
    /// the import transaction fails.
    pub fn import_legacy_project_database(
        &self,
        source_path: impl AsRef<Path>,
        workspace: impl AsRef<Path>,
    ) -> SessionStoreResult<LocalStoreImportReport> {
        let source_path = source_path.as_ref();
        if !source_path.exists() {
            return Ok(LocalStoreImportReport {
                source_path: source_path.to_path_buf(),
                workspace: normalize_workspace(workspace.as_ref()),
                sessions_imported: 0,
                rows_imported: 0,
                imported: false,
            });
        }
        let source_path = fs::canonicalize(source_path).map_err(map_io_error)?;
        let workspace = normalize_workspace(workspace.as_ref());

        let target_path = {
            let connection = self.lock()?;
            main_database_path(&connection)?
        };
        if target_path
            .as_deref()
            .and_then(|path| fs::canonicalize(path).ok())
            .as_ref()
            == Some(&source_path)
        {
            return Ok(LocalStoreImportReport {
                source_path,
                workspace,
                sessions_imported: 0,
                rows_imported: 0,
                imported: false,
            });
        }

        migrate_sqlite_database(&source_path)?;
        let source_key = source_path.to_string_lossy().into_owned();
        let mut connection = self.lock()?;
        connection
            .execute("ATTACH DATABASE ?1 AS legacy_store", [&source_key])
            .map_err(map_sqlite_session_error)?;
        let result = import_attached_database(&mut connection, &source_key, &workspace);
        let detach_result = connection.execute("DETACH DATABASE legacy_store", []);
        match (result, detach_result) {
            (Ok(report), Ok(_)) => Ok(LocalStoreImportReport {
                source_path,
                workspace,
                sessions_imported: report.0,
                rows_imported: report.1,
                imported: report.1 > 0,
            }),
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(map_sqlite_session_error(error)),
        }
    }
}

fn import_attached_database(
    connection: &mut rusqlite::Connection,
    source_key: &str,
    workspace: &str,
) -> SessionStoreResult<(usize, usize)> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(map_sqlite_session_error)?;
    transaction
        .execute_batch(
            "CREATE TEMP TABLE newly_imported_session_ids (
                 session_id TEXT PRIMARY KEY
             ) WITHOUT ROWID;
             CREATE TEMP TABLE imported_session_ids (
                 session_id TEXT PRIMARY KEY
             ) WITHOUT ROWID;",
        )
        .map_err(map_sqlite_session_error)?;
    transaction
        .execute(
            "INSERT INTO newly_imported_session_ids (session_id)
             SELECT source.session_id
             FROM legacy_store.session_records AS source
             LEFT JOIN main.session_records AS target
               ON target.session_id = source.session_id
             LEFT JOIN main.local_store_import_tombstones AS tombstone
               ON tombstone.source_path = ?1 AND tombstone.session_id = source.session_id
             WHERE target.session_id IS NULL AND tombstone.session_id IS NULL",
            [source_key],
        )
        .map_err(map_sqlite_session_error)?;

    // Bootstrap per-session provenance for databases imported by the earlier path-level marker.
    // Deserialize records rather than depending on SQLite JSON extensions or envelope layout.
    let mut provenance_session_ids = Vec::new();
    {
        let mut statement = transaction
            .prepare(
                "SELECT target.session_id, target.record
                 FROM main.session_records AS target
                 JOIN legacy_store.session_records AS source
                   ON source.session_id = target.session_id
                 LEFT JOIN local_store_import_sessions AS tracked
                   ON tracked.source_path = ?1 AND tracked.session_id = target.session_id
                 WHERE tracked.session_id IS NULL",
            )
            .map_err(map_sqlite_session_error)?;
        let rows = statement
            .query_map([source_key], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(map_sqlite_session_error)?;
        for row in rows {
            let (session_id, payload) = row.map_err(map_sqlite_session_error)?;
            let session = deserialize_json_record::<SessionRecord>(&payload)?;
            if session
                .metadata
                .get(SESSION_IMPORTED_FROM_METADATA_KEY)
                .and_then(Value::as_str)
                == Some(source_key)
            {
                provenance_session_ids.push(session_id);
            }
        }
    }
    for session_id in provenance_session_ids {
        transaction
            .execute(
                "INSERT OR IGNORE INTO local_store_import_sessions
                 (source_path, session_id, imported_at) VALUES (?1, ?2, ?3)",
                params![source_key, session_id, Utc::now().to_rfc3339()],
            )
            .map_err(map_sqlite_session_error)?;
    }

    let mut sessions = Vec::new();
    {
        let mut statement = transaction
            .prepare(
                "SELECT source.record
                 FROM legacy_store.session_records AS source
                 JOIN newly_imported_session_ids AS imported
                   ON imported.session_id = source.session_id",
            )
            .map_err(map_sqlite_session_error)?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(map_sqlite_session_error)?;
        for row in rows {
            let payload = row.map_err(map_sqlite_session_error)?;
            sessions.push(deserialize_json_record::<SessionRecord>(&payload)?);
        }
    }

    for session in &mut sessions {
        if session.workspace.is_none() {
            session.workspace = Some(workspace.to_string());
        }
        session.metadata.insert(
            SESSION_SOURCE_PRODUCT_METADATA_KEY.to_string(),
            Value::String("cli".to_string()),
        );
        session.metadata.insert(
            SESSION_IMPORTED_FROM_METADATA_KEY.to_string(),
            Value::String(source_key.to_string()),
        );
        save_session_record(&transaction, session)?;
        transaction
            .execute(
                "INSERT OR IGNORE INTO local_store_import_sessions
                 (source_path, session_id, imported_at) VALUES (?1, ?2, ?3)",
                params![
                    source_key,
                    session.session_id.as_str(),
                    Utc::now().to_rfc3339()
                ],
            )
            .map_err(map_sqlite_session_error)?;
    }
    transaction
        .execute(
            "INSERT INTO imported_session_ids (session_id)
             SELECT tracked.session_id
             FROM local_store_import_sessions AS tracked
             JOIN legacy_store.session_records AS source
               ON source.session_id = tracked.session_id
             JOIN main.session_records AS target
               ON target.session_id = tracked.session_id
             WHERE tracked.source_path = ?1",
            [source_key],
        )
        .map_err(map_sqlite_session_error)?;

    // Synchronize mutable session records only after source ownership has been proven by the
    // per-session provenance table. This lets an old project-local CLI terminalize a run after its
    // first import without allowing an unrelated destination collision to overwrite canonical data.
    let mut sessions_updated = 0;
    let mut tracked_sessions = Vec::new();
    {
        let mut statement = transaction
            .prepare(
                "SELECT source.record
                 FROM legacy_store.session_records AS source
                 JOIN imported_session_ids AS imported
                   ON imported.session_id = source.session_id",
            )
            .map_err(map_sqlite_session_error)?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(map_sqlite_session_error)?;
        for row in rows {
            tracked_sessions.push(deserialize_json_record::<SessionRecord>(
                &row.map_err(map_sqlite_session_error)?,
            )?);
        }
    }
    for session in &mut tracked_sessions {
        if session.workspace.is_none() {
            session.workspace = Some(workspace.to_string());
        }
        session.metadata.insert(
            SESSION_SOURCE_PRODUCT_METADATA_KEY.to_string(),
            Value::String("cli".to_string()),
        );
        session.metadata.insert(
            SESSION_IMPORTED_FROM_METADATA_KEY.to_string(),
            Value::String(source_key.to_string()),
        );
        let current_payload = transaction
            .query_row(
                "SELECT record FROM session_records WHERE session_id = ?1",
                [session.session_id.as_str()],
                |row| row.get::<_, String>(0),
            )
            .map_err(map_sqlite_session_error)?;
        let current = deserialize_json_record::<SessionRecord>(&current_payload)?;
        if current != *session {
            save_session_record(&transaction, session)?;
            sessions_updated += 1;
        }
    }
    let mut rows_imported = sessions.len() + sessions_updated;

    for sql in SESSION_SCOPED_COPY_STATEMENTS {
        rows_imported += transaction
            .execute(sql, [])
            .map_err(map_sqlite_session_error)?;
    }
    for sql in REPLAY_SCOPED_COPY_STATEMENTS {
        rows_imported += transaction
            .execute(sql, [])
            .map_err(map_sqlite_session_error)?;
    }

    transaction
        .execute(
            "INSERT INTO local_store_imports
             (source_path, workspace, sessions_imported, rows_imported, imported_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(source_path) DO UPDATE SET
               workspace = excluded.workspace,
               sessions_imported = local_store_imports.sessions_imported + excluded.sessions_imported,
               rows_imported = local_store_imports.rows_imported + excluded.rows_imported,
               imported_at = excluded.imported_at",
            params![
                source_key,
                workspace,
                i64::try_from(sessions.len()).map_err(map_display_error)?,
                i64::try_from(rows_imported).map_err(map_display_error)?,
                Utc::now().to_rfc3339(),
            ],
        )
        .map_err(map_sqlite_session_error)?;
    transaction
        .execute_batch("DROP TABLE imported_session_ids; DROP TABLE newly_imported_session_ids;")
        .map_err(map_sqlite_session_error)?;
    transaction.commit().map_err(map_sqlite_session_error)?;
    Ok((sessions.len(), rows_imported))
}

const SESSION_SCOPED_COPY_STATEMENTS: &[&str] = &[
    "INSERT INTO run_records
       (session_id, run_id, record, sequence_no, created_at, updated_at)
     SELECT source.session_id, source.run_id, source.record, source.sequence_no,
            source.created_at, source.updated_at
     FROM legacy_store.run_records AS source
     JOIN imported_session_ids AS imported USING (session_id)
     WHERE true
     ON CONFLICT(session_id, run_id) DO UPDATE SET
       record = excluded.record,
       sequence_no = excluded.sequence_no,
       created_at = excluded.created_at,
       updated_at = excluded.updated_at
     WHERE run_records.record IS NOT excluded.record
        OR run_records.sequence_no IS NOT excluded.sequence_no
        OR run_records.created_at IS NOT excluded.created_at
        OR run_records.updated_at IS NOT excluded.updated_at",
    "INSERT OR IGNORE INTO checkpoint_records
     SELECT source.* FROM legacy_store.checkpoint_records AS source
     JOIN imported_session_ids AS imported USING (session_id)",
    "INSERT OR IGNORE INTO stream_records
     SELECT source.* FROM legacy_store.stream_records AS source
     JOIN imported_session_ids AS imported USING (session_id)",
    "INSERT INTO approval_records
       (session_id, run_id, approval_id, record, updated_at)
     SELECT source.session_id, source.run_id, source.approval_id, source.record, source.updated_at
     FROM legacy_store.approval_records AS source
     JOIN imported_session_ids AS imported USING (session_id)
     WHERE true
     ON CONFLICT(session_id, run_id, approval_id) DO UPDATE SET
       record = excluded.record,
       updated_at = excluded.updated_at
     WHERE approval_records.record IS NOT excluded.record
        OR approval_records.updated_at IS NOT excluded.updated_at",
    "INSERT INTO deferred_tool_records
       (session_id, run_id, deferred_id, record, updated_at)
     SELECT source.session_id, source.run_id, source.deferred_id, source.record, source.updated_at
     FROM legacy_store.deferred_tool_records AS source
     JOIN imported_session_ids AS imported USING (session_id)
     WHERE true
     ON CONFLICT(session_id, run_id, deferred_id) DO UPDATE SET
       record = excluded.record,
       updated_at = excluded.updated_at
     WHERE deferred_tool_records.record IS NOT excluded.record
        OR deferred_tool_records.updated_at IS NOT excluded.updated_at",
    "INSERT INTO run_context_records (session_id, run_id, record, updated_at)
     SELECT source.session_id, source.run_id, source.record, source.updated_at
     FROM legacy_store.run_context_records AS source
     JOIN imported_session_ids AS imported USING (session_id)
     WHERE true
     ON CONFLICT(session_id, run_id) DO UPDATE SET
       record = excluded.record,
       updated_at = excluded.updated_at
     WHERE run_context_records.record IS NOT excluded.record
        OR run_context_records.updated_at IS NOT excluded.updated_at",
    "INSERT INTO run_environment_records (session_id, run_id, record, updated_at)
     SELECT source.session_id, source.run_id, source.record, source.updated_at
     FROM legacy_store.run_environment_records AS source
     JOIN imported_session_ids AS imported USING (session_id)
     WHERE true
     ON CONFLICT(session_id, run_id) DO UPDATE SET
       record = excluded.record,
       updated_at = excluded.updated_at
     WHERE run_environment_records.record IS NOT excluded.record
        OR run_environment_records.updated_at IS NOT excluded.updated_at",
    "INSERT INTO run_evidence_commits (session_id, run_id, digest, created_at)
     SELECT source.session_id, source.run_id, source.digest, source.created_at
     FROM legacy_store.run_evidence_commits AS source
     JOIN imported_session_ids AS imported USING (session_id)
     WHERE true
     ON CONFLICT(session_id, run_id) DO UPDATE SET
       digest = excluded.digest,
       created_at = excluded.created_at
     WHERE run_evidence_commits.digest IS NOT excluded.digest
        OR run_evidence_commits.created_at IS NOT excluded.created_at",
];

const REPLAY_SCOPED_COPY_STATEMENTS: &[&str] = &[
    "INSERT OR IGNORE INTO display_message_records
     SELECT source.* FROM legacy_store.display_message_records AS source
     WHERE source.scope IN (
       SELECT 'run:' || runs.run_id FROM legacy_store.run_records AS runs
       JOIN imported_session_ids AS imported USING (session_id)
       UNION ALL SELECT 'session:' || imported.session_id FROM imported_session_ids AS imported
     )",
    "INSERT INTO display_snapshot_records (scope, record, updated_at)
     SELECT source.scope, source.record, source.updated_at
     FROM legacy_store.display_snapshot_records AS source
     WHERE source.scope IN (
       SELECT 'run:' || runs.run_id FROM legacy_store.run_records AS runs
       JOIN imported_session_ids AS imported USING (session_id)
       UNION ALL SELECT 'session:' || imported.session_id FROM imported_session_ids AS imported
     )
     ON CONFLICT(scope) DO UPDATE SET
       record = excluded.record,
       updated_at = excluded.updated_at
     WHERE display_snapshot_records.record IS NOT excluded.record
        OR display_snapshot_records.updated_at IS NOT excluded.updated_at",
    "INSERT OR IGNORE INTO replay_events
     SELECT source.* FROM legacy_store.replay_events AS source
     WHERE source.scope IN (
       SELECT 'run:' || runs.run_id FROM legacy_store.run_records AS runs
       JOIN imported_session_ids AS imported USING (session_id)
       UNION ALL SELECT 'session:' || imported.session_id FROM imported_session_ids AS imported
     )",
    "INSERT INTO replay_snapshot_records (scope, record, updated_at)
     SELECT source.scope, source.record, source.updated_at
     FROM legacy_store.replay_snapshot_records AS source
     WHERE source.scope IN (
       SELECT 'run:' || runs.run_id FROM legacy_store.run_records AS runs
       JOIN imported_session_ids AS imported USING (session_id)
       UNION ALL SELECT 'session:' || imported.session_id FROM imported_session_ids AS imported
     )
     ON CONFLICT(scope) DO UPDATE SET
       record = excluded.record,
       updated_at = excluded.updated_at
     WHERE replay_snapshot_records.record IS NOT excluded.record
        OR replay_snapshot_records.updated_at IS NOT excluded.updated_at",
];

fn main_database_path(connection: &rusqlite::Connection) -> SessionStoreResult<Option<PathBuf>> {
    connection
        .query_row("PRAGMA database_list", [], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, String>(2)?))
        })
        .optional()
        .map(|entry| {
            entry.and_then(|(name, path)| {
                (name == "main" && !path.is_empty()).then(|| PathBuf::from(path))
            })
        })
        .map_err(map_sqlite_session_error)
}

fn normalize_workspace(path: &Path) -> String {
    fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

fn map_io_error(error: std::io::Error) -> SessionStoreError {
    SessionStoreError::Failed(error.to_string())
}

fn map_display_error(error: impl std::fmt::Display) -> SessionStoreError {
    SessionStoreError::Failed(error.to_string())
}

#[cfg(test)]
mod tests {
    use serde_json::Value;
    use starweaver_core::{ConversationId, RunId};
    use starweaver_session::{RunRecord, RunStatus};
    use starweaver_stream::{DisplayMessage, DisplayMessageKind, ReplayScope, StreamArchive};

    use super::*;
    use crate::session_store::records::save_run_record;

    #[tokio::test]
    async fn legacy_import_is_incremental_idempotent_and_preserves_target_ownership() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let source_path = tempdir.path().join("project.sqlite");
        let target_path = tempdir.path().join("canonical.sqlite");
        let workspace = tempdir.path().join("workspace");
        fs::create_dir_all(&workspace).expect("workspace");

        let source = SqliteStorage::open(&source_path).expect("source store");
        let session = source
            .create_session(Some("general".to_string()), Some("legacy".to_string()))
            .expect("legacy session");
        let run_id = RunId::from_string("run_legacy_import");
        let run = source
            .begin_run(RunRecord::new(
                session.session_id.clone(),
                run_id.clone(),
                ConversationId::new(),
            ))
            .expect("legacy run");
        let display = DisplayMessage::new(
            1,
            session.session_id.clone(),
            run_id.clone(),
            DisplayMessageKind::RunCompleted,
        )
        .with_preview("legacy output");
        source
            .stream_archive()
            .append_display_messages(ReplayScope::run(run_id.as_str()), vec![display.clone()])
            .await
            .expect("legacy display");

        let collision = source
            .create_session(
                Some("source-profile".to_string()),
                Some("source collision".to_string()),
            )
            .expect("source collision session");
        source
            .begin_run(RunRecord::new(
                collision.session_id.clone(),
                RunId::from_string("run_source_collision"),
                ConversationId::new(),
            ))
            .expect("source collision run");
        source
            .lock()
            .expect("source connection")
            .execute(
                "INSERT INTO run_admission_generations
                 (namespace_id, session_id, generation) VALUES ('local', ?1, 7)",
                [session.session_id.as_str()],
            )
            .expect("source process-control row");
        source
            .lock()
            .expect("source connection")
            .execute(
                "INSERT INTO hitl_resume_claims
                 (session_id, run_id, claim_id, record, created_at)
                 VALUES (?1, ?2, 'stale-legacy-claim', '{\"state\":\"started\"}', ?3)",
                params![
                    session.session_id.as_str(),
                    run_id.as_str(),
                    Utc::now().to_rfc3339()
                ],
            )
            .expect("source stale HITL claim");
        drop(source);

        let target = SqliteStorage::open(&target_path).expect("target store");
        let mut target_collision = SessionRecord::new(collision.session_id.clone());
        target_collision.title = Some("target collision".to_string());
        save_session_record(
            &target.lock().expect("target connection"),
            &target_collision,
        )
        .expect("target collision session");

        let first = target
            .import_legacy_project_database(&source_path, &workspace)
            .expect("first import");
        assert!(first.imported);
        assert_eq!(first.sessions_imported, 1);
        assert!(first.rows_imported >= 3);

        let imported = target
            .load_session(&session.session_id)
            .expect("imported session");
        assert_eq!(
            imported.workspace.as_deref(),
            Some(first.workspace.as_str())
        );
        assert_eq!(
            imported.metadata.get(SESSION_SOURCE_PRODUCT_METADATA_KEY),
            Some(&Value::String("cli".to_string()))
        );
        assert_eq!(
            imported.metadata.get(SESSION_IMPORTED_FROM_METADATA_KEY),
            Some(&Value::String(
                fs::canonicalize(&source_path)
                    .expect("canonical source")
                    .to_string_lossy()
                    .into_owned()
            ))
        );
        assert_eq!(
            target.list_runs(&session.session_id).expect("runs"),
            vec![run]
        );
        assert_eq!(
            target
                .stream_archive()
                .replay_display_after(&ReplayScope::run(run_id.as_str()), None)
                .await
                .expect("display replay"),
            vec![display.clone()]
        );
        assert_eq!(
            target
                .load_session(&collision.session_id)
                .expect("target collision survives")
                .title
                .as_deref(),
            Some("target collision")
        );
        assert!(
            target
                .list_runs(&collision.session_id)
                .expect("collision runs")
                .is_empty(),
            "a target-owned collision must not inherit source evidence"
        );
        assert_eq!(
            target
                .lock()
                .expect("target connection")
                .query_row(
                    "SELECT COUNT(*) FROM run_admission_generations",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .expect("target process-control count"),
            0,
            "process-control tables must not be imported"
        );

        assert_eq!(
            target
                .lock()
                .expect("target connection")
                .query_row("SELECT COUNT(*) FROM hitl_resume_claims", [], |row| {
                    row.get::<_, i64>(0)
                })
                .expect("target HITL claim count"),
            0,
            "exclusive HITL resume claims must not be imported"
        );

        let second = target
            .import_legacy_project_database(&source_path, &workspace)
            .expect("no-op retry");
        assert!(!second.imported);
        assert_eq!(second.sessions_imported, 0);
        assert_eq!(second.rows_imported, 0);

        let source = SqliteStorage::open(&source_path).expect("reopen source store");
        let mut terminal_run = source
            .load_run(&session.session_id, &run_id)
            .expect("load legacy run for terminal update");
        terminal_run.status = RunStatus::Completed;
        terminal_run.output_preview = Some("terminal legacy output".to_string());
        terminal_run.updated_at = Utc::now();
        save_run_record(&source.lock().expect("source connection"), &terminal_run)
            .expect("terminalize legacy run");
        let mut terminal_session = source
            .load_session(&session.session_id)
            .expect("load legacy session for terminal update");
        terminal_session.title = Some("legacy terminalized".to_string());
        terminal_session.head_run_id = Some(run_id.clone());
        terminal_session.head_success_run_id = Some(run_id.clone());
        terminal_session.active_run_id = None;
        terminal_session.updated_at = terminal_run.updated_at;
        save_session_record(
            &source.lock().expect("source connection"),
            &terminal_session,
        )
        .expect("update legacy session head");
        let later_display = DisplayMessage::new(
            2,
            session.session_id.clone(),
            run_id.clone(),
            DisplayMessageKind::RunCompleted,
        )
        .with_preview("later legacy output");
        source
            .stream_archive()
            .append_display_messages(
                ReplayScope::run(run_id.as_str()),
                vec![later_display.clone()],
            )
            .await
            .expect("later legacy display");
        let later_session = source
            .create_session(
                Some("general".to_string()),
                Some("later legacy session".to_string()),
            )
            .expect("later legacy session");
        let later_run_id = RunId::from_string("run_later_legacy_import");
        let later_run = source
            .begin_run(RunRecord::new(
                later_session.session_id.clone(),
                later_run_id.clone(),
                ConversationId::new(),
            ))
            .expect("later legacy run");
        let later_session_display = DisplayMessage::new(
            1,
            later_session.session_id.clone(),
            later_run_id.clone(),
            DisplayMessageKind::RunCompleted,
        )
        .with_preview("later session output");
        source
            .stream_archive()
            .append_display_messages(
                ReplayScope::run(later_run_id.as_str()),
                vec![later_session_display.clone()],
            )
            .await
            .expect("later session display");
        drop(source);

        let incremental = target
            .import_legacy_project_database(&source_path, &workspace)
            .expect("incremental import");
        assert!(incremental.imported);
        assert_eq!(incremental.sessions_imported, 1);
        assert!(incremental.rows_imported >= 6);
        let synchronized_session = target
            .load_session(&session.session_id)
            .expect("synchronized legacy session");
        assert_eq!(
            synchronized_session.title.as_deref(),
            Some("legacy terminalized")
        );
        assert_eq!(synchronized_session.head_run_id.as_ref(), Some(&run_id));
        assert_eq!(
            synchronized_session.head_success_run_id.as_ref(),
            Some(&run_id)
        );
        assert!(synchronized_session.active_run_id.is_none());
        let synchronized_run = target
            .load_run(&session.session_id, &run_id)
            .expect("synchronized legacy run");
        assert_eq!(synchronized_run.status, RunStatus::Completed);
        assert_eq!(
            synchronized_run.output_preview.as_deref(),
            Some("terminal legacy output")
        );
        assert_eq!(
            target
                .stream_archive()
                .replay_display_after(&ReplayScope::run(run_id.as_str()), None)
                .await
                .expect("incremental display replay"),
            vec![display, later_display]
        );
        assert_eq!(
            target
                .list_runs(&later_session.session_id)
                .expect("later imported runs"),
            vec![later_run]
        );
        assert_eq!(
            target
                .stream_archive()
                .replay_display_after(&ReplayScope::run(later_run_id.as_str()), None)
                .await
                .expect("later session display replay"),
            vec![later_session_display]
        );
        assert!(
            target
                .list_runs(&collision.session_id)
                .expect("collision runs after incremental import")
                .is_empty(),
            "a target-owned collision must remain excluded on later imports"
        );

        let final_retry = target
            .import_legacy_project_database(&source_path, &workspace)
            .expect("final no-op retry");
        assert!(!final_retry.imported);
        assert_eq!(final_retry.sessions_imported, 0);
        assert_eq!(final_retry.rows_imported, 0);

        assert!(
            target
                .delete_session(&session.session_id)
                .expect("delete import")
        );
        let after_delete = target
            .import_legacy_project_database(&source_path, &workspace)
            .expect("import after physical delete");
        assert!(!after_delete.imported);
        assert_eq!(after_delete.sessions_imported, 0);
        assert!(target.load_session(&session.session_id).is_err());
        assert_eq!(
            target
                .lock()
                .expect("target connection")
                .query_row(
                    "SELECT COUNT(*) FROM local_store_import_tombstones
                     WHERE session_id = ?1",
                    [session.session_id.as_str()],
                    |row| row.get::<_, i64>(0),
                )
                .expect("import tombstone count"),
            1
        );
    }
}
