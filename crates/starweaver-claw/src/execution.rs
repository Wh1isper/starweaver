//! Single-node execution supervision primitives.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use starweaver_core::RunId;
use starweaver_session::{RunRecord, RunStatus, SessionFilter, SessionStore};
use starweaver_stream::{
    ReplayEvent, ReplayEventKind, ReplayEventLog, ReplayScope, StreamTerminalMarker,
};
use tokio::{sync::Semaphore, task::JoinSet};

use crate::{ClawError, ClawResult, ClawRuntimeState};

/// Result returned by a run executor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionOutput {
    /// Output preview or final text.
    pub output_text: String,
}

impl ExecutionOutput {
    /// Build an execution output.
    #[must_use]
    pub fn new(output_text: impl Into<String>) -> Self {
        Self {
            output_text: output_text.into(),
        }
    }
}

/// Pluggable execution backend for one queued run.
#[async_trait]
pub trait RunExecutor: Send + Sync {
    /// Execute one run.
    async fn execute(&self, run: RunRecord) -> ClawResult<ExecutionOutput>;
}

/// Deterministic placeholder executor used before the SDK runtime adapter is wired.
#[derive(Clone, Debug, Default)]
pub struct NoopRunExecutor;

#[async_trait]
impl RunExecutor for NoopRunExecutor {
    async fn execute(&self, run: RunRecord) -> ClawResult<ExecutionOutput> {
        let input_count = run.input.len();
        Ok(ExecutionOutput::new(format!(
            "run {} accepted with {input_count} input part(s)",
            run.run_id.as_str()
        )))
    }
}

/// Single-node execution supervisor.
#[derive(Clone)]
pub struct ExecutionSupervisor {
    store: Arc<dyn SessionStore>,
    events: Arc<dyn ReplayEventLog>,
    runtime_state: ClawRuntimeState,
    executor: Arc<dyn RunExecutor>,
    max_concurrency: usize,
}

impl ExecutionSupervisor {
    /// Build a supervisor.
    #[must_use]
    pub fn new(
        store: Arc<dyn SessionStore>,
        events: Arc<dyn ReplayEventLog>,
        runtime_state: ClawRuntimeState,
        executor: Arc<dyn RunExecutor>,
    ) -> Self {
        Self {
            store,
            events,
            runtime_state,
            executor,
            max_concurrency: 4,
        }
    }

    /// Set maximum concurrent run executions.
    #[must_use]
    pub const fn with_max_concurrency(mut self, max_concurrency: usize) -> Self {
        self.max_concurrency = max_concurrency;
        self
    }

    /// Recover all queued runs and execute them once.
    ///
    /// # Errors
    ///
    /// Returns storage or event errors from run dispatch.
    pub async fn recover_queued_runs(&self) -> ClawResult<Vec<String>> {
        let sessions = self.store.list_sessions(SessionFilter::default()).await?;
        let mut queued_runs = Vec::new();
        for session in sessions {
            let runs = self.store.list_runs(&session.session_id).await?;
            queued_runs.extend(
                runs.into_iter()
                    .filter(|run| run.status == RunStatus::Queued),
            );
        }

        let semaphore = Arc::new(Semaphore::new(self.max_concurrency.max(1)));
        let mut join_set = JoinSet::new();
        let run_ids = queued_runs
            .iter()
            .map(|run| run.run_id.as_str().to_string())
            .collect::<Vec<_>>();
        for run in queued_runs {
            let permit = semaphore
                .clone()
                .acquire_owned()
                .await
                .map_err(|error| ClawError::Failed(error.to_string()))?;
            let supervisor = self.clone();
            join_set.spawn(async move {
                let _permit = permit;
                supervisor.execute_run(run).await
            });
        }
        while let Some(result) = join_set.join_next().await {
            result.map_err(|error| ClawError::Failed(error.to_string()))??;
        }
        Ok(run_ids)
    }

    /// Execute one queued run.
    ///
    /// # Errors
    ///
    /// Returns storage, event, or executor errors.
    pub async fn execute_run(&self, run: RunRecord) -> ClawResult<()> {
        let run_id = run.run_id.clone();
        let session_id = run.session_id.clone();
        self.runtime_state
            .register_run(
                session_id.as_str().to_string(),
                run_id.as_str().to_string(),
                "async",
            )
            .await;
        self.store
            .update_run_status(
                &session_id,
                &run_id,
                RunStatus::Running,
                run.output_preview.clone(),
            )
            .await?;
        self.append_run_event(
            &run_id,
            2,
            ReplayEventKind::Raw(json!({
                "type": "run.running",
                "run_id": run_id.as_str(),
                "session_id": session_id.as_str(),
            })),
        )
        .await?;

        match self.executor.execute(run).await {
            Ok(output) => {
                self.store
                    .update_run_status(
                        &session_id,
                        &run_id,
                        RunStatus::Completed,
                        Some(output.output_text.clone()),
                    )
                    .await?;
                self.append_run_event(
                    &run_id,
                    3,
                    ReplayEventKind::Raw(json!({
                        "type": "run.completed",
                        "run_id": run_id.as_str(),
                        "session_id": session_id.as_str(),
                        "output_text": output.output_text,
                    })),
                )
                .await?;
                self.append_run_event(
                    &run_id,
                    4,
                    ReplayEventKind::Terminal(StreamTerminalMarker::RunCompleted),
                )
                .await?;
                self.runtime_state.close_run(run_id.as_str()).await;
                Ok(())
            }
            Err(error) => {
                self.store
                    .update_run_status(
                        &session_id,
                        &run_id,
                        RunStatus::Failed,
                        Some(error.to_string()),
                    )
                    .await?;
                self.append_run_event(
                    &run_id,
                    3,
                    ReplayEventKind::Terminal(StreamTerminalMarker::RunFailed {
                        code: "execution_failed".to_string(),
                        message: error.to_string(),
                    }),
                )
                .await?;
                self.runtime_state.close_run(run_id.as_str()).await;
                Err(error)
            }
        }
    }

    async fn append_run_event(
        &self,
        run_id: &RunId,
        sequence: usize,
        kind: ReplayEventKind,
    ) -> ClawResult<()> {
        let scope = ReplayScope::run(run_id.as_str());
        self.events
            .append(scope.clone(), ReplayEvent::new(scope, sequence, kind))
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use starweaver_core::{ConversationId, SessionId};
    use starweaver_session::{InMemorySessionStore, SessionRecord};
    use starweaver_stream::InMemoryReplayEventLog;

    #[tokio::test]
    async fn supervisor_completes_queued_runs() {
        let store = Arc::new(InMemorySessionStore::new());
        let events = Arc::new(InMemoryReplayEventLog::new());
        let session_id = SessionId::from_string("session_exec");
        store
            .save_session(SessionRecord::new(session_id.clone()))
            .await
            .expect("save session");
        let run_id = RunId::from_string("run_exec");
        let mut run = RunRecord::new(session_id.clone(), run_id.clone(), ConversationId::new());
        run.sequence_no = 1;
        store.append_run(run).await.expect("append run");

        let supervisor = ExecutionSupervisor::new(
            store.clone(),
            events,
            ClawRuntimeState::new(),
            Arc::new(NoopRunExecutor),
        );
        let recovered = supervisor
            .recover_queued_runs()
            .await
            .expect("recover queued runs");
        assert_eq!(recovered, vec!["run_exec".to_string()]);
        let run = store
            .load_run(&session_id, &run_id)
            .await
            .expect("load run");
        assert_eq!(run.status, RunStatus::Completed);
    }
}
