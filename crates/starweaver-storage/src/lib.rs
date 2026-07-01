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

mod migrations;
mod replay_log;
mod schema;
mod session_store;
mod sqlite;
mod stream_archive;

pub use migrations::{
    SqliteAppliedMigration, SqliteMigrationStatus, SqlitePendingMigration, migrate_sqlite_database,
    sqlite_migration_status,
};
pub use replay_log::SqliteReplayEventLog;
pub use session_store::SqliteSessionStore;
pub use stream_archive::SqliteStreamArchive;

#[cfg(test)]
mod tests {
    use rusqlite::{Connection, params};
    use starweaver_core::{ConversationId, SessionId};
    use starweaver_session::{RunRecord, SessionRecord, SessionStore};
    use starweaver_stream::{ReplayEventKind, ReplayEventLog, ReplayScope};

    use super::*;
    use crate::schema::SQLITE_MIGRATIONS;

    #[test]
    fn sqlite_migrations_are_idempotent() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let database_path = tempdir.path().join("storage.sqlite3");
        let first = migrate_sqlite_database(&database_path).expect("first migration");
        assert_eq!(first, vec!["20260605_000001_session_stream_store"]);
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
        "checkpoints",
        "stream_records",
        "approvals",
        "deferred_tools",
        "replay_events",
        "replay_snapshots",
    ];

    const FOUNDATION_INDEXES: &[&str] = &[
        "ix_run_records_session_sequence",
        "ix_replay_events_scope_sequence",
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
        assert!(migrated_status.current);
        assert_eq!(migrated_status.applied.len(), SQLITE_MIGRATIONS.len());
        assert!(migrated_status.pending.is_empty());
        assert_eq!(
            migrated_status.latest_migration,
            SQLITE_MIGRATIONS.last().map(|migration| migration.id)
        );
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
    use starweaver_runtime::{AgentStreamEvent, AgentStreamRecord};
    use starweaver_stream::{
        DisplayMessage, DisplayMessageKind, ReplayCursor, ReplayEvent, ReplayEventKind,
        ReplayEventLog, ReplayScope, ReplaySnapshot, StreamArchive,
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
                Some(ReplayCursor::new(ReplayScope::run(run_id.as_str()), 1)),
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
            cursor: Some(ReplayCursor::new(scope.clone(), 10)),
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
