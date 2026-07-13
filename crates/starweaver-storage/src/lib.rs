#![allow(
    clippy::cast_possible_truncation,
    clippy::derive_partial_eq_without_eq,
    clippy::doc_markdown,
    clippy::double_must_use,
    clippy::expect_used,
    clippy::map_unwrap_or,
    clippy::match_same_arms,
    clippy::missing_const_for_fn,
    clippy::missing_errors_doc,
    clippy::format_push_string,
    clippy::module_name_repetitions,
    clippy::needless_pass_by_value,
    clippy::redundant_clone,
    clippy::significant_drop_tightening,
    clippy::struct_excessive_bools,
    clippy::too_many_lines
)]
//! Shared SQLite storage adapters and migrations for Starweaver.

mod blocking;
pub(crate) mod domain;
mod migrations;
mod replay_log;
mod schema;
mod session_store;
mod sqlite;
mod storage;
mod stream_archive;

pub use migrations::{
    SqliteAppliedMigration, SqliteMigrationStatus, SqlitePendingMigration, migrate_sqlite_database,
    sqlite_migration_status,
};
pub use replay_log::SqliteReplayEventLog;
pub use session_store::SqliteSessionStore;
pub use starweaver_session::RunEvidenceCommit;
pub use storage::SqliteStorage;
pub use stream_archive::SqliteStreamArchive;

#[cfg(test)]
mod contract_fixtures_tests;
#[cfg(test)]
mod domain_tests;

#[cfg(test)]
mod tests {
    use rusqlite::{Connection, params};
    use starweaver_context::{AgentCheckpoint, AgentRunState};
    use starweaver_core::{AgentExecutionNode, ConversationId, Metadata, RunId, SessionId};
    use starweaver_session::{RunRecord, SessionRecord, SessionStore};
    use starweaver_stream::{
        AgentStreamEvent, AgentStreamRecord, ReplayEventKind, ReplayEventLog, ReplayScope,
    };

    use super::*;
    use crate::schema::SQLITE_MIGRATIONS;

    #[test]
    fn sqlite_migrations_are_idempotent() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let database_path = tempdir.path().join("storage.sqlite3");
        let first = migrate_sqlite_database(&database_path).expect("first migration");
        assert_eq!(
            first,
            vec![
                "20260605_000001_session_stream_store",
                "20260711_000002_namespaced_evidence_tables",
                "20260711_000003_split_display_and_replay_families",
                "20260712_000004_evidence_outbox_and_resume_claims",
            ]
        );
        let second = migrate_sqlite_database(&database_path).expect("second migration");
        assert!(second.is_empty());

        let connection = Connection::open(database_path).expect("open migrated database");
        for table in FOUNDATION_TABLES {
            assert_table_exists(&connection, table);
        }
        for index in FOUNDATION_INDEXES {
            assert_index_exists(&connection, index);
        }
    }

    const FOUNDATION_TABLES: &[&str] = &[
        "session_records",
        "run_records",
        "checkpoint_records",
        "stream_records",
        "approval_records",
        "deferred_tool_records",
        "replay_events",
        "display_message_records",
        "display_snapshot_records",
        "replay_snapshot_records",
        "run_context_records",
        "run_environment_records",
        "run_evidence_commits",
        "stream_publication_outbox",
        "hitl_resume_claims",
    ];

    const FOUNDATION_INDEXES: &[&str] = &[
        "ix_run_records_session_sequence",
        "ix_replay_events_scope_sequence",
        "ix_display_message_records_scope_sequence",
        "ix_stream_publication_outbox_session",
    ];

    fn assert_table_exists(connection: &Connection, table: &str) {
        let count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                params![table],
                |row| row.get(0),
            )
            .expect("count table");
        assert_eq!(count, 1, "missing table {table}");
    }

    fn assert_index_exists(connection: &Connection, index: &str) {
        let count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = ?1",
                params![index],
                |row| row.get(0),
            )
            .expect("count index");
        assert_eq!(count, 1, "missing index {index}");
    }

    #[test]
    fn sqlite_migration_status_is_reported() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let status_path = tempdir.path().join("status.sqlite3");
        let initial_status = sqlite_migration_status(&status_path).expect("initial status");
        assert!(!initial_status.migration_table_exists);
        assert!(!initial_status.current);
        assert_eq!(initial_status.pending.len(), SQLITE_MIGRATIONS.len());

        migrate_sqlite_database(&status_path).expect("migrate status database");
        let migrated_status = sqlite_migration_status(&status_path).expect("migrated status");
        assert!(migrated_status.migration_table_exists);
        assert!(migrated_status.checksums_valid());
        assert!(migrated_status.current);
        assert_eq!(migrated_status.applied.len(), SQLITE_MIGRATIONS.len());
        assert!(migrated_status.pending.is_empty());
        assert_eq!(
            migrated_status.latest_migration,
            SQLITE_MIGRATIONS.last().map(|migration| migration.id)
        );
    }

    #[test]
    fn sqlite_migration_checksum_mismatch_is_rejected() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let database_path = tempdir.path().join("checksum.sqlite3");
        migrate_sqlite_database(&database_path).expect("migrate database");
        let connection = Connection::open(&database_path).expect("open database");
        connection
            .execute(
                "UPDATE starweaver_schema_migrations SET checksum = 'tampered' WHERE id = ?1",
                [SQLITE_MIGRATIONS[0].id],
            )
            .expect("tamper checksum");
        drop(connection);

        let status = sqlite_migration_status(&database_path).expect("migration status");
        assert!(!status.checksums_valid());
        assert!(!status.current);
        let error = migrate_sqlite_database(&database_path).expect_err("checksum mismatch");
        assert!(error.to_string().contains("migration checksum mismatch"));
    }

    #[tokio::test]
    async fn sqlite_store_round_trips_session_and_run() {
        let store = SqliteSessionStore::in_memory().expect("sqlite store");
        let session_id = SessionId::from_string("session_test");
        store
            .save_session(SessionRecord::new(session_id.clone()))
            .await
            .expect("save session");
        let run_id = starweaver_core::RunId::from_string("run_test");
        let mut run = RunRecord::new(session_id.clone(), run_id.clone(), ConversationId::new());
        run.sequence_no = 1;
        store.append_run(run).await.expect("append run");

        let session = store.load_session(&session_id).await.expect("load session");
        assert_eq!(session.active_run_id.as_ref(), Some(&run_id));
        let runs = store.list_runs(&session_id).await.expect("list runs");
        assert_eq!(runs.len(), 1);
    }

    #[tokio::test]
    async fn append_run_rolls_back_when_session_update_fails() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let database_path = tempdir.path().join("atomic.sqlite3");
        let store = SqliteSessionStore::open(&database_path).expect("sqlite store");
        let session_id = SessionId::from_string("session_atomic");
        store
            .save_session(SessionRecord::new(session_id.clone()))
            .await
            .expect("save session");
        let connection = Connection::open(&database_path).expect("open trigger connection");
        connection
            .execute_batch(
                "CREATE TRIGGER fail_session_update
                 BEFORE UPDATE ON session_records
                 BEGIN
                   SELECT RAISE(ABORT, 'injected session update failure');
                 END;",
            )
            .expect("create failure trigger");
        drop(connection);

        let run_id = starweaver_core::RunId::from_string("run_atomic");
        let mut run = RunRecord::new(session_id.clone(), run_id, ConversationId::new());
        run.sequence_no = 1;
        let error = store.append_run(run).await.expect_err("append must fail");
        assert!(
            error
                .to_string()
                .contains("injected session update failure")
        );
        let runs = store.list_runs(&session_id).await.expect("list runs");
        assert!(
            runs.is_empty(),
            "run insert must roll back with session update"
        );
    }

    #[tokio::test]
    async fn append_checkpoint_rolls_back_when_run_update_fails() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let database_path = tempdir.path().join("checkpoint_atomic.sqlite3");
        let store = SqliteSessionStore::open(&database_path).expect("sqlite store");
        let session_id = SessionId::from_string("session_checkpoint_atomic");
        let run_id = RunId::from_string("run_checkpoint_atomic");
        store
            .save_session(SessionRecord::new(session_id.clone()))
            .await
            .expect("save session");
        let mut run = RunRecord::new(session_id.clone(), run_id.clone(), ConversationId::new());
        run.sequence_no = 1;
        store.append_run(run).await.expect("append run");

        let connection = Connection::open(&database_path).expect("open trigger connection");
        connection
            .execute_batch(
                "CREATE TRIGGER fail_checkpoint_run_update
                 BEFORE UPDATE ON run_records
                 BEGIN
                   SELECT RAISE(ABORT, 'injected checkpoint run update failure');
                 END;",
            )
            .expect("create failure trigger");
        drop(connection);

        let state = AgentRunState::new(run_id.clone(), ConversationId::new());
        let checkpoint = AgentCheckpoint::new(AgentExecutionNode::RunStart, &state);
        let error = store
            .append_checkpoint(&session_id, checkpoint)
            .await
            .expect_err("checkpoint append must fail");
        assert!(
            error
                .to_string()
                .contains("injected checkpoint run update failure")
        );
        assert!(
            store
                .load_checkpoints(&session_id, &run_id)
                .await
                .expect("load checkpoints")
                .is_empty(),
            "checkpoint insert must roll back with run update"
        );
    }

    #[tokio::test]
    async fn append_checkpoint_is_idempotent_and_rejects_conflicts() {
        let store = SqliteSessionStore::in_memory().expect("sqlite store");
        let session_id = SessionId::from_string("session_checkpoint_idempotent");
        let run_id = RunId::from_string("run_checkpoint_idempotent");
        store
            .save_session(SessionRecord::new(session_id.clone()))
            .await
            .expect("save session");
        let mut run = RunRecord::new(session_id.clone(), run_id.clone(), ConversationId::new());
        run.sequence_no = 1;
        store.append_run(run).await.expect("append run");

        let state = AgentRunState::new(run_id.clone(), ConversationId::new());
        let checkpoint = AgentCheckpoint::new(AgentExecutionNode::RunStart, &state);
        store
            .append_checkpoint(&session_id, checkpoint.clone())
            .await
            .expect("append checkpoint");
        store
            .append_checkpoint(&session_id, checkpoint.clone())
            .await
            .expect("idempotent checkpoint retry");
        assert_eq!(
            store
                .load_checkpoints(&session_id, &run_id)
                .await
                .expect("load checkpoints"),
            vec![checkpoint.clone()]
        );

        let conflicting = checkpoint.clone().with_metadata(Metadata::from_iter([(
            "different".to_string(),
            serde_json::json!(true),
        )]));
        let error = store
            .append_checkpoint(&session_id, conflicting)
            .await
            .expect_err("conflicting checkpoint must fail");
        assert!(error.to_string().contains("checkpoint conflict"));

        let mut changed_sequence = checkpoint;
        changed_sequence.run_step = changed_sequence.run_step.saturating_add(1);
        let error = store
            .append_checkpoint(&session_id, changed_sequence)
            .await
            .expect_err("checkpoint identity with changed sequence must fail");
        assert!(error.to_string().contains("checkpoint conflict"));
    }

    #[tokio::test]
    async fn append_stream_batch_rolls_back_when_cursor_update_fails() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let database_path = tempdir.path().join("stream_atomic.sqlite3");
        let store = SqliteSessionStore::open(&database_path).expect("sqlite store");
        let session_id = SessionId::from_string("session_stream_atomic");
        let run_id = RunId::from_string("run_stream_atomic");
        store
            .save_session(SessionRecord::new(session_id.clone()))
            .await
            .expect("save session");
        let mut run = RunRecord::new(session_id.clone(), run_id.clone(), ConversationId::new());
        run.sequence_no = 1;
        store.append_run(run).await.expect("append run");

        let connection = Connection::open(&database_path).expect("open trigger connection");
        connection
            .execute_batch(
                "CREATE TRIGGER fail_stream_cursor_update
                 BEFORE UPDATE ON run_records
                 BEGIN
                   SELECT RAISE(ABORT, 'injected stream cursor update failure');
                 END;",
            )
            .expect("create failure trigger");
        drop(connection);

        let record = AgentStreamRecord::new(
            1,
            AgentStreamEvent::RunComplete {
                run_id: run_id.clone(),
                output: "done".to_string(),
            },
        );
        let error = store
            .append_stream_records(&session_id, &run_id, vec![record])
            .await
            .expect_err("stream append must fail");
        assert!(
            error
                .to_string()
                .contains("injected stream cursor update failure")
        );
        assert!(
            store
                .replay_stream_records(&session_id, &run_id)
                .await
                .expect("replay stream records")
                .is_empty(),
            "stream rows must roll back with cursor update"
        );
        let run = store
            .load_run(&session_id, &run_id)
            .await
            .expect("load run");
        assert!(run.stream_cursors.is_empty());
        let session = store.load_session(&session_id).await.expect("load session");
        assert!(session.stream_cursors.is_empty());
    }

    #[tokio::test]
    async fn sqlite_replay_log_round_trips_events() {
        let log = SqliteReplayEventLog::in_memory().expect("replay log");
        let scope = ReplayScope::run("run_test");
        log.append(
            scope.clone(),
            starweaver_stream::ReplayEvent::new(scope.clone(), 1, ReplayEventKind::Heartbeat),
        )
        .await
        .expect("append event");
        let events = log.replay_after(&scope, None, None).await.expect("replay");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].sequence, 1);
    }
}

#[cfg(test)]
mod replay_tests {
    use starweaver_core::{Metadata, RunId, SessionId};
    use starweaver_stream::{
        AgentStreamEvent, AgentStreamRecord, DisplayMessage, DisplayMessageKind, ReplayCursor,
        ReplayEvent, ReplayEventKind, ReplayEventLog, ReplayScope, ReplaySnapshot, StreamArchive,
    };

    use super::*;

    #[tokio::test]
    async fn sqlite_stream_archive_round_trips_raw_display_and_snapshots() {
        let archive = SqliteStreamArchive::in_memory().expect("stream archive");
        let session_id = SessionId::from_string("session_archive");
        let run_id = RunId::from_string("run_archive");
        archive
            .append_raw_records(
                &session_id,
                &run_id,
                vec![
                    AgentStreamRecord::new(
                        1,
                        AgentStreamEvent::RunComplete {
                            run_id: run_id.clone(),
                            output: "done".to_string(),
                        },
                    ),
                    AgentStreamRecord::new(
                        2,
                        AgentStreamEvent::RunComplete {
                            run_id: run_id.clone(),
                            output: "done again".to_string(),
                        },
                    ),
                ],
            )
            .await
            .expect("append raw records");
        let raw = archive
            .replay_raw_after(
                &session_id,
                &run_id,
                Some(ReplayCursor::raw_runtime(
                    ReplayScope::run(run_id.as_str()),
                    1,
                )),
            )
            .await
            .expect("replay raw records");
        assert_eq!(raw.len(), 1);
        assert_eq!(raw[0].sequence, 2);

        let scope = ReplayScope::run(run_id.as_str());
        let display = DisplayMessage::new(
            10,
            session_id.clone(),
            run_id.clone(),
            DisplayMessageKind::RunCompleted,
        )
        .with_preview("done");
        archive
            .append_display_messages(scope.clone(), vec![display.clone()])
            .await
            .expect("append display");
        archive
            .append_display_messages(scope.clone(), vec![display.clone()])
            .await
            .expect("idempotent display append");
        let conflicting_display = DisplayMessage::new(
            10,
            session_id.clone(),
            run_id.clone(),
            DisplayMessageKind::RunCompleted,
        )
        .with_preview("different");
        let error = archive
            .append_display_messages(scope.clone(), vec![conflicting_display])
            .await
            .expect_err("conflicting display append");
        assert!(error.to_string().contains("replay event conflict"));
        let messages = archive
            .replay_display_after(&scope, None)
            .await
            .expect("replay display");
        assert_eq!(messages, vec![display.clone()]);
        let range = archive
            .cursor_range(&scope)
            .await
            .expect("cursor range")
            .expect("range exists");
        assert_eq!(range.0.sequence, 10);
        assert_eq!(range.1.sequence, 10);

        let snapshot = ReplaySnapshot {
            scope: Some(scope.clone()),
            revision: 1,
            cursor: Some(ReplayCursor::display(scope.clone(), 10)),
            display_messages: vec![display],
            metadata: Metadata::default(),
        };
        archive
            .append_snapshot(scope.clone(), snapshot.clone())
            .await
            .expect("append snapshot");
        let loaded = archive
            .latest_snapshot(&scope)
            .await
            .expect("load snapshot")
            .expect("snapshot exists");
        assert_eq!(loaded, snapshot);
    }

    #[tokio::test]
    async fn sqlite_replay_log_round_trips_events() {
        let log = SqliteReplayEventLog::in_memory().expect("replay log");
        let scope = ReplayScope::run("run_test");
        log.append(
            scope.clone(),
            ReplayEvent::new(scope.clone(), 1, ReplayEventKind::Heartbeat),
        )
        .await
        .expect("append event");
        let events = log.replay_after(&scope, None, None).await.expect("replay");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].sequence, 1);
    }

    #[tokio::test]
    async fn sqlite_replay_log_unbounded_replay_keeps_all_events() {
        let log = SqliteReplayEventLog::in_memory().expect("replay log");
        let scope = ReplayScope::run("run_unbounded");
        for sequence in 1..=1005 {
            log.append(
                scope.clone(),
                ReplayEvent::new(scope.clone(), sequence, ReplayEventKind::Heartbeat),
            )
            .await
            .expect("append event");
        }

        let all_events = log
            .replay_after(&scope, None, None)
            .await
            .expect("unbounded replay");
        assert_eq!(all_events.len(), 1005);
        assert_eq!(all_events[0].sequence, 1);
        assert_eq!(all_events[1004].sequence, 1005);

        let limited_events = log
            .replay_after(&scope, None, Some(10))
            .await
            .expect("bounded replay");
        assert_eq!(limited_events.len(), 10);
        assert_eq!(limited_events[9].sequence, 10);
    }

    #[tokio::test]
    async fn sqlite_replay_log_live_subscription_receives_appended_events() {
        let log = SqliteReplayEventLog::in_memory().expect("replay log");
        let scope = ReplayScope::run("run_live");
        let mut subscription = log
            .subscribe(scope.clone(), None)
            .await
            .expect("subscribe replay log");
        log.append(
            scope.clone(),
            ReplayEvent::new(scope.clone(), 1, ReplayEventKind::Heartbeat),
        )
        .await
        .expect("append event");

        let event = tokio::time::timeout(std::time::Duration::from_secs(1), subscription.recv())
            .await
            .expect("live event arrives")
            .expect("live event");
        assert_eq!(event.scope, scope);
        assert_eq!(event.sequence, 1);
    }

    #[tokio::test]
    async fn sqlite_replay_log_duplicate_sequence_is_idempotent_and_conflict_safe() {
        let log = SqliteReplayEventLog::in_memory().expect("replay log");
        let scope = ReplayScope::run("run_duplicate");
        let event = ReplayEvent::new(scope.clone(), 1, ReplayEventKind::Heartbeat);
        let mut subscription = log
            .subscribe(scope.clone(), None)
            .await
            .expect("subscribe replay log");

        log.append(scope.clone(), event.clone())
            .await
            .expect("first append");
        log.append(scope.clone(), event.clone())
            .await
            .expect("idempotent append");

        let live = tokio::time::timeout(std::time::Duration::from_secs(1), subscription.recv())
            .await
            .expect("first live event arrives")
            .expect("first live event");
        assert_eq!(live, event);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), subscription.recv())
                .await
                .is_err(),
            "idempotent append must not publish a duplicate live event"
        );

        let conflict = ReplayEvent::new(
            scope.clone(),
            1,
            ReplayEventKind::Raw(serde_json::json!({"different": true})),
        );
        let error = log
            .append(scope.clone(), conflict)
            .await
            .expect_err("conflicting duplicate must fail");
        assert!(error.to_string().contains("replay event conflict"));
        let persisted = log
            .replay_after(&scope, None, None)
            .await
            .expect("replay persisted event");
        assert_eq!(persisted, vec![event]);
    }

    #[tokio::test]
    async fn reopened_subscription_catches_up_from_durable_backlog_without_reappend() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let database_path = tempdir.path().join("replay-reconcile.sqlite3");
        let scope = ReplayScope::run("run_reconcile");
        let event = ReplayEvent::new(scope.clone(), 1, ReplayEventKind::Heartbeat);

        let first = SqliteReplayEventLog::open(&database_path).expect("first replay log");
        first
            .append(scope.clone(), event.clone())
            .await
            .expect("persist event");
        drop(first);

        let reopened = SqliteReplayEventLog::open(&database_path).expect("reopened replay log");
        let mut subscription = reopened
            .subscribe(scope.clone(), None)
            .await
            .expect("subscribe with durable backlog");
        let published =
            tokio::time::timeout(std::time::Duration::from_secs(1), subscription.recv())
                .await
                .expect("durable backlog event arrives")
                .expect("durable backlog event");
        assert_eq!(published, event);
    }
}

#[cfg(test)]
mod agent_runtime_tests {
    use std::sync::Arc;

    use starweaver_agent::{AgentRuntimeBuilder, TestModel};
    use starweaver_core::SessionId;
    use starweaver_session::{RunStatus, SessionStore};
    use starweaver_stream::{ReplayEventKind, ReplayEventLog, ReplayScope, StreamArchive};

    use super::*;

    #[tokio::test]
    async fn sqlite_storage_adapters_back_agent_runtime_facade() {
        let store = Arc::new(SqliteSessionStore::in_memory().expect("session store"));
        let archive = Arc::new(SqliteStreamArchive::in_memory().expect("stream archive"));
        let replay = Arc::new(SqliteReplayEventLog::in_memory().expect("replay log"));
        let session_id = SessionId::from_string("session_sqlite_runtime");
        let mut runtime = AgentRuntimeBuilder::new(Arc::new(TestModel::with_text("ok")))
            .durable_session_id(session_id.clone())
            .session_store(store.clone())
            .stream_archive(archive.clone())
            .replay_event_log(replay.clone())
            .build();

        let result = runtime.run("hello sqlite").await.expect("run");
        let run_id = result.state.run_id.clone();
        let session = store.load_session(&session_id).await.expect("load session");
        let run = store
            .load_run(&session_id, &run_id)
            .await
            .expect("load run");

        assert_eq!(session.head_success_run_id.as_ref(), Some(&run_id));
        assert_eq!(run.status, RunStatus::Completed);
        assert_eq!(run.input.len(), 1);
        assert!(
            !store
                .load_checkpoints(&session_id, &run_id)
                .await
                .expect("load checkpoints")
                .is_empty()
        );
        assert!(
            !archive
                .replay_raw_after(&session_id, &run_id, None)
                .await
                .expect("replay raw")
                .is_empty()
        );
        let replay_events = replay
            .replay_after(&ReplayScope::run(run_id.as_str()), None, None)
            .await
            .expect("replay events");
        assert!(
            replay_events
                .iter()
                .any(|event| matches!(event.event, ReplayEventKind::Terminal { .. }))
        );
        let snapshot = runtime
            .resume_snapshot(&session_id, &run_id)
            .await
            .expect("resume snapshot");
        assert_eq!(snapshot.run.run_id, run_id);
        assert!(snapshot.latest_checkpoint.is_some());
    }
}
