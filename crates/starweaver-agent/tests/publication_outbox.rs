//! Production-path stream-publication outbox regression test.

#![allow(clippy::expect_used)]

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use async_trait::async_trait;
use starweaver_agent::{AgentRuntimeBuilder, SessionId, TestModel};
use starweaver_session::{InMemorySessionStore, SessionStore};
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
