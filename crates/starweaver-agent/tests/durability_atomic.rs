//! Production-path atomic durability regression tests.

#![allow(clippy::expect_used)]

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use async_trait::async_trait;
use rusqlite::Connection;
use starweaver_agent::{
    AgentRuntimeBuilder, FunctionTool, SessionId, StaticToolset, TestModel, ToolContext, ToolResult,
};
use starweaver_model::tool_call_response;
use starweaver_session::{InMemorySessionStore, RunStatus, SessionStore};
use starweaver_storage::SqliteStorage;
use starweaver_stream::{
    AgentStreamRecord, DisplayMessage, InMemoryReplayEventLog, InMemoryStreamArchive, ReplayCursor,
    ReplayError, ReplayEventLog, ReplayResult, ReplayScope, ReplaySnapshot, StreamArchive,
};

struct FailOnceDisplayArchive {
    inner: InMemoryStreamArchive,
    fail_display: AtomicBool,
}

impl FailOnceDisplayArchive {
    fn new() -> Self {
        Self {
            inner: InMemoryStreamArchive::new(),
            fail_display: AtomicBool::new(true),
        }
    }
}

#[async_trait]
impl StreamArchive for FailOnceDisplayArchive {
    async fn append_raw_records(
        &self,
        session_id: &starweaver_agent::SessionId,
        run_id: &starweaver_core::RunId,
        records: Vec<AgentStreamRecord>,
    ) -> ReplayResult<()> {
        self.inner
            .append_raw_records(session_id, run_id, records)
            .await
    }

    async fn replay_raw_after(
        &self,
        session_id: &starweaver_agent::SessionId,
        run_id: &starweaver_core::RunId,
        cursor: Option<ReplayCursor>,
    ) -> ReplayResult<Vec<AgentStreamRecord>> {
        self.inner
            .replay_raw_after(session_id, run_id, cursor)
            .await
    }

    async fn append_display_messages(
        &self,
        scope: ReplayScope,
        messages: Vec<DisplayMessage>,
    ) -> ReplayResult<()> {
        if self.fail_display.swap(false, Ordering::SeqCst) {
            return Err(ReplayError::Failed(
                "injected display publication failure".to_string(),
            ));
        }
        self.inner.append_display_messages(scope, messages).await
    }

    async fn replay_display_after(
        &self,
        scope: &ReplayScope,
        cursor: Option<ReplayCursor>,
    ) -> ReplayResult<Vec<DisplayMessage>> {
        self.inner.replay_display_after(scope, cursor).await
    }

    async fn append_snapshot(
        &self,
        scope: ReplayScope,
        snapshot: ReplaySnapshot,
    ) -> ReplayResult<()> {
        self.inner.append_snapshot(scope, snapshot).await
    }

    async fn latest_snapshot(&self, scope: &ReplayScope) -> ReplayResult<Option<ReplaySnapshot>> {
        self.inner.latest_snapshot(scope).await
    }

    async fn cursor_range(
        &self,
        scope: &ReplayScope,
    ) -> ReplayResult<Option<(ReplayCursor, ReplayCursor)>> {
        self.inner.cursor_range(scope).await
    }
}

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
async fn committed_run_keeps_failed_publication_in_outbox_and_retries_it() {
    let store = Arc::new(InMemorySessionStore::new());
    let archive = Arc::new(FailOnceDisplayArchive::new());
    let replay = Arc::new(InMemoryReplayEventLog::new());
    let session_id = SessionId::from_string("session-runtime-publication-outbox");
    let mut runtime = AgentRuntimeBuilder::new(Arc::new(TestModel::with_text("complete")))
        .durable_session_id(session_id.clone())
        .session_store(store.clone())
        .stream_archive(archive.clone())
        .replay_event_log(replay.clone())
        .build();

    let result = runtime
        .run_stream("publish reliably")
        .await
        .expect("sink failure must not invalidate committed run");
    let pending = store
        .pending_stream_publications(&session_id)
        .await
        .expect("pending outbox");
    assert_eq!(pending.len(), 1);
    assert!(pending[0].archive_pending);
    assert!(!pending[0].replay_pending);
    let scope = ReplayScope::run(result.result.state.run_id.as_str());
    assert!(
        !replay
            .replay_after(&scope, None, None)
            .await
            .expect("independent replay publication")
            .is_empty()
    );

    runtime
        .flush_pending_stream_publications()
        .await
        .expect("retry pending publication");
    assert!(
        store
            .pending_stream_publications(&session_id)
            .await
            .expect("drained outbox")
            .is_empty()
    );
    assert!(
        !archive
            .replay_raw_after(&session_id, &result.result.state.run_id, None)
            .await
            .expect("raw archive")
            .is_empty()
    );
    assert!(
        !archive
            .replay_display_after(&scope, None)
            .await
            .expect("display archive")
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
