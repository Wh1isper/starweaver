//! Production-path atomic storage durability regression tests.

#![allow(clippy::expect_used)]

use std::sync::Arc;

use rusqlite::Connection;
use starweaver_agent::{
    AgentRuntimeBuilder, FunctionTool, SessionId, StaticToolset, TestModel, ToolContext, ToolResult,
};
use starweaver_model::tool_call_response;
use starweaver_session::{RunStatus, SessionStore};
use starweaver_storage::SqliteStorage;
use starweaver_stream::{ReplayEventLog, ReplayScope, StreamArchive};

#[tokio::test]
async fn production_runtime_rolls_back_complete_evidence_when_late_write_fails() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let database_path = tempdir.path().join("runtime-atomic.sqlite3");
    let storage = SqliteStorage::open(&database_path).expect("storage");
    let session_store = Arc::new(storage.session_store());
    let archive = Arc::new(storage.stream_archive());
    let replay = Arc::new(storage.replay_event_log());
    let session_id = SessionId::from_string("session-runtime-atomic");

    let connection = Connection::open(&database_path).expect("trigger connection");
    connection
        .execute_batch(
            "CREATE TRIGGER fail_runtime_evidence
             BEFORE INSERT ON replay_events
             BEGIN
               SELECT RAISE(ABORT, 'injected runtime evidence failure');
             END;",
        )
        .expect("install failure trigger");
    drop(connection);

    let mut runtime = AgentRuntimeBuilder::new(Arc::new(TestModel::with_text("complete")))
        .durable_session_id(session_id.clone())
        .session_store(session_store.clone())
        .stream_archive(archive.clone())
        .replay_event_log(replay.clone())
        .build();

    let error = runtime
        .run_stream("persist atomically")
        .await
        .expect_err("late evidence write must fail");
    assert!(
        error
            .to_string()
            .contains("injected runtime evidence failure")
    );

    let runs = session_store
        .list_runs(&session_id)
        .await
        .expect("list checkpoint-created run");
    assert_eq!(runs.len(), 1);
    let run = &runs[0];
    assert_ne!(run.status, RunStatus::Completed);
    let scope = ReplayScope::run(run.run_id.as_str());
    assert!(
        archive
            .replay_raw_after(&session_id, &run.run_id, None)
            .await
            .expect("raw replay")
            .is_empty()
    );
    assert!(
        archive
            .replay_display_after(&scope, None)
            .await
            .expect("display replay")
            .is_empty()
    );
    assert!(
        replay
            .replay_after(&scope, None, None)
            .await
            .expect("event replay")
            .is_empty()
    );
}

#[tokio::test]
async fn durable_run_failure_commits_pending_hitl_evidence() {
    let storage = SqliteStorage::in_memory().expect("storage");
    let session_store = Arc::new(storage.session_store());
    let session_id = SessionId::from_string("session-runtime-failure-hitl");
    let dangerous = FunctionTool::new(
        "dangerous",
        Some("Requires approval".to_string()),
        serde_json::json!({"type": "object"}),
        |_context: ToolContext, _arguments| async move {
            Ok(ToolResult::new(serde_json::json!({"executed": true})))
        },
    );
    let toolset: starweaver_agent::DynToolset =
        Arc::new(StaticToolset::new("dangerous-tools").with_tool(Arc::new(dangerous)));
    let model = TestModel::with_responses(vec![tool_call_response(
        "approval-call",
        "dangerous",
        serde_json::json!({}),
    )]);
    let mut runtime = AgentRuntimeBuilder::new(Arc::new(model))
        .durable_session_id(session_id.clone())
        .session_store(session_store.clone())
        .approval_required_tools(["dangerous"])
        .toolset(&toolset)
        .build();

    let waiting = runtime.run("request approval").await.expect("waiting run");
    assert_eq!(waiting.state.status, starweaver_agent::RunStatus::Waiting);

    runtime
        .run("fail after waiting")
        .await
        .expect_err("exhausted model response must fail");

    let runs = session_store
        .list_runs(&session_id)
        .await
        .expect("list runs");
    let failed = runs
        .iter()
        .find(|run| run.status == RunStatus::Failed)
        .expect("failed run must be committed");
    let approvals = session_store
        .load_approvals(&session_id, &failed.run_id)
        .await
        .expect("load failed-run approvals");
    assert_eq!(approvals.len(), 1);
    assert_eq!(approvals[0].action_name, "dangerous");
}
