//! Session store trait, executor adapter, and built-in implementations.

mod contract;
mod memory;

use std::sync::Arc;

use async_trait::async_trait;
use starweaver_core::SessionId;
use starweaver_runtime::AgentCheckpoint;

use crate::{
    error::SessionStoreError,
    records::{RunRecord, RunStatus, SessionRecord},
};

pub use contract::{SessionFilter, SessionStore};
pub use memory::InMemorySessionStore;

/// Executor adapter that persists runtime checkpoints into a session store.
#[derive(Clone)]
pub struct SessionStoreExecutor {
    store: Arc<dyn SessionStore>,
    session_id: SessionId,
}

impl SessionStoreExecutor {
    /// Create a checkpoint executor for one session.
    #[must_use]
    pub fn new(store: Arc<dyn SessionStore>, session_id: SessionId) -> Self {
        Self { store, session_id }
    }

    /// Return the session id associated with this executor.
    #[must_use]
    pub const fn session_id(&self) -> &SessionId {
        &self.session_id
    }
}

#[async_trait]
impl starweaver_runtime::AgentExecutor for SessionStoreExecutor {
    async fn checkpoint(
        &self,
        checkpoint: AgentCheckpoint,
    ) -> Result<starweaver_runtime::AgentExecutionDecision, starweaver_runtime::AgentExecutorError>
    {
        match self
            .store
            .append_checkpoint(&self.session_id, checkpoint.clone())
            .await
        {
            Ok(()) => {}
            Err(SessionStoreError::NotFound(_)) => {
                match self.store.load_session(&self.session_id).await {
                    Ok(_) => {}
                    Err(SessionStoreError::NotFound(_)) => {
                        self.store
                            .save_session(SessionRecord::new(self.session_id.clone()))
                            .await?;
                    }
                    Err(error) => return Err(error.into()),
                }
                let mut run = RunRecord::new(
                    self.session_id.clone(),
                    checkpoint.run_id.clone(),
                    checkpoint.conversation_id.clone(),
                );
                run.status = checkpoint_status(checkpoint.resume.status);
                run.trace_context = checkpoint.resume.trace_context.clone();
                run.parent_run_id = checkpoint.state.parent_run_id.clone();
                run.parent_task_id = checkpoint.state.parent_task_id.clone();
                self.store.append_run(run).await?;
                self.store
                    .append_checkpoint(&self.session_id, checkpoint)
                    .await?;
            }
            Err(error) => return Err(error.into()),
        }
        Ok(starweaver_runtime::AgentExecutionDecision::Continue)
    }
}

const fn checkpoint_status(status: starweaver_runtime::RunStatus) -> RunStatus {
    match status {
        starweaver_runtime::RunStatus::Starting | starweaver_runtime::RunStatus::Running => {
            RunStatus::Running
        }
        starweaver_runtime::RunStatus::Waiting => RunStatus::Waiting,
        starweaver_runtime::RunStatus::Completed => RunStatus::Completed,
        starweaver_runtime::RunStatus::Failed => RunStatus::Failed,
        starweaver_runtime::RunStatus::Cancelled => RunStatus::Cancelled,
    }
}
