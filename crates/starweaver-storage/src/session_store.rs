//! SQLite-backed durable session store adapter.

use std::{
    path::Path,
    sync::{Arc, Mutex, MutexGuard},
};

use rusqlite::Connection;
use starweaver_session::{SessionStoreError, SessionStoreResult};

use crate::{SqliteStorage, blocking::BlockingOperationTracker, sqlite::SharedSqliteConnection};

mod background;
pub mod host_events;
mod impl_store;
mod management;
pub use management::ensure_run_admission_in_transaction;
pub mod records;
mod trace_helpers;

/// SQLite-backed durable session store.
#[derive(Clone, Debug)]
pub struct SqliteSessionStore {
    connection: Arc<Mutex<Connection>>,
    background_operations: BlockingOperationTracker,
}

impl SqliteSessionStore {
    /// Open or create a SQLite session store.
    ///
    /// # Errors
    ///
    /// Returns a store error when SQLite cannot open or initialize the database.
    pub fn open(path: impl AsRef<Path>) -> SessionStoreResult<Self> {
        Ok(SqliteStorage::open(path)?.session_store())
    }

    /// Open an in-memory SQLite session store.
    ///
    /// # Errors
    ///
    /// Returns a store error when SQLite cannot initialize the database.
    pub fn in_memory() -> SessionStoreResult<Self> {
        Ok(SqliteStorage::in_memory()?.session_store())
    }

    pub(crate) fn from_shared(connection: SharedSqliteConnection) -> Self {
        Self {
            connection,
            background_operations: BlockingOperationTracker::default(),
        }
    }

    fn lock(&self) -> SessionStoreResult<MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|error| SessionStoreError::Failed(error.to_string()))
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::too_many_lines, clippy::unwrap_used)]
mod tests {
    use std::time::Duration;

    use chrono::Utc;
    use starweaver_core::{ConversationId, RunId, SessionId, SubagentAttemptId};
    use starweaver_session::{
        BACKGROUND_SUBAGENT_RECORD_VERSION, BackgroundSubagentArtifact, BackgroundSubagentRecord,
        BackgroundSubagentTerminalCommit, DurableBackgroundSubagentDeliveryStatus,
        DurableBackgroundSubagentExecutionStatus, DurableBackgroundSubagentOwnerLease,
        DurableBackgroundSubagentResultRef, DurableBackgroundSubagentRetentionStatus, RunRecord,
        SessionRecord, SessionStore,
    };

    use super::SqliteSessionStore;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dropped_sqlite_terminal_caller_remains_tracked_and_replays_exactly() {
        let store = SqliteSessionStore::in_memory().unwrap();
        let session_id = SessionId::from_string("session-tracked-terminal");
        let run_id = RunId::from_string("run-tracked-terminal");
        let mut session = SessionRecord::new(session_id.clone());
        session.namespace_id = "test".to_string();
        store.save_session(session).await.unwrap();
        store
            .append_run(RunRecord::new(
                session_id.clone(),
                run_id.clone(),
                ConversationId::from_string("conversation-tracked-terminal"),
            ))
            .await
            .unwrap();

        let accepted_at = Utc::now();
        let attempt_id = SubagentAttemptId::from_string("subattempt-tracked-terminal");
        let acceptance = BackgroundSubagentRecord {
            schema_version: BACKGROUND_SUBAGENT_RECORD_VERSION,
            attempt_id: attempt_id.clone(),
            agent_id: "child-bg-tracked-terminal".to_string(),
            linked_task_id: None,
            subagent_name: "child".to_string(),
            namespace_id: "test".to_string(),
            parent_session_id: session_id,
            parent_run_id: run_id,
            child_run_id: None,
            continuation_run_id: None,
            profile: "test-profile".to_string(),
            owner_lease: DurableBackgroundSubagentOwnerLease {
                host_instance_id: "test-host".to_string(),
                fencing_generation: 1,
                heartbeat_at: accepted_at,
                lease_expires_at: accepted_at + chrono::Duration::minutes(1),
            },
            execution_status: DurableBackgroundSubagentExecutionStatus::Accepted,
            result_ref: None,
            failure_category: None,
            cancellation_reason: None,
            delivery_status: DurableBackgroundSubagentDeliveryStatus::Undelivered,
            delivery_claim: None,
            delivered_claim_id: None,
            automatic_continuation_suppressed_by_run_id: None,
            retention_status: DurableBackgroundSubagentRetentionStatus::Inline,
            retention_expires_at: None,
            trace_context: None,
            accepted_at,
            updated_at: accepted_at,
            terminal_at: None,
        };
        store
            .record_background_subagent_acceptance(acceptance.clone())
            .await
            .unwrap();
        let mut running = acceptance.clone();
        running.execution_status = DurableBackgroundSubagentExecutionStatus::Starting;
        running.updated_at = Utc::now();
        running = store
            .update_background_subagent_execution(running)
            .await
            .unwrap();
        running.execution_status = DurableBackgroundSubagentExecutionStatus::Running;
        running.updated_at = Utc::now();
        running = store
            .update_background_subagent_execution(running)
            .await
            .unwrap();
        let terminal_at = Utc::now();
        let content = "tracked terminal result";
        let mut terminal = running;
        terminal.execution_status = DurableBackgroundSubagentExecutionStatus::Completed;
        terminal.result_ref = Some(DurableBackgroundSubagentResultRef {
            content: Some(content.to_string()),
            error: None,
            artifact_ref: None,
            digest: Some(BackgroundSubagentArtifact::content_digest(content)),
            size_bytes: u64::try_from(content.len()).unwrap(),
        });
        terminal.retention_expires_at = Some(terminal_at + chrono::Duration::hours(1));
        terminal.updated_at = terminal_at;
        terminal.terminal_at = Some(terminal_at);
        let commit = BackgroundSubagentTerminalCommit {
            record: terminal.clone(),
            artifact: None,
            artifact_limits: None,
        };

        let connection = store.connection.clone();
        let (locked_tx, locked_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let locker = std::thread::spawn(move || {
            let _guard = connection.lock().expect("SQLite connection lock");
            locked_tx.send(()).expect("locked signal");
            release_rx.recv().expect("release signal");
        });
        locked_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("connection must be locked");

        let started_operations = store.background_operations.started();
        let caller_store = store.clone();
        let caller_commit = commit.clone();
        let caller = tokio::spawn(async move {
            caller_store
                .commit_background_subagent_terminal(caller_commit)
                .await
        });
        tokio::time::timeout(Duration::from_secs(1), async {
            while store.background_operations.active() == 0
                || store.background_operations.started() == started_operations
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("terminal operation must start and remain tracked");
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        assert!(
            tokio::time::timeout(
                Duration::from_millis(20),
                store.drain_background_subagent_operations(),
            )
            .await
            .is_err()
        );

        release_tx.send(()).expect("release SQLite lock");
        locker.join().expect("SQLite lock thread");
        tokio::time::timeout(
            Duration::from_secs(1),
            store.drain_background_subagent_operations(),
        )
        .await
        .expect("terminal operation must drain")
        .unwrap();
        let persisted = store.load_background_subagent(&attempt_id).await.unwrap();
        assert_eq!(persisted, terminal);
        assert_eq!(
            store
                .commit_background_subagent_terminal(commit)
                .await
                .unwrap(),
            terminal
        );
    }
}
