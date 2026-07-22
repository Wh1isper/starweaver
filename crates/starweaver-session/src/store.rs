//! Session store trait, executor adapter, and built-in implementations.

mod contract;
mod memory;

use std::sync::Arc;

use async_trait::async_trait;
use starweaver_context::{
    AgentCheckpoint, AgentExecutionDecision, AgentExecutor, AgentExecutorError,
};
use starweaver_core::SessionId;

use crate::RunAdmissionLease;

pub use contract::{
    InteractionPage, InteractionPageKey, InteractionPageQuery, MAX_STABLE_PAGE_SIZE, SessionFilter,
    SessionPage, SessionPageKey, SessionPageQuery, SessionStore,
};
pub use memory::InMemorySessionStore;

/// Executor adapter that persists runtime checkpoints into a session store.
#[derive(Clone)]
pub struct SessionStoreExecutor {
    store: Arc<dyn SessionStore>,
    session_id: SessionId,
    admission_lease: Option<RunAdmissionLease>,
}

impl SessionStoreExecutor {
    /// Create a checkpoint executor for one session.
    #[must_use]
    pub fn new(store: Arc<dyn SessionStore>, session_id: SessionId) -> Self {
        Self {
            store,
            session_id,
            admission_lease: None,
        }
    }

    /// Create a checkpoint executor fenced to one active run admission.
    #[must_use]
    pub fn new_fenced(
        store: Arc<dyn SessionStore>,
        session_id: SessionId,
        admission_lease: RunAdmissionLease,
    ) -> Self {
        Self {
            store,
            session_id,
            admission_lease: Some(admission_lease),
        }
    }

    /// Return the session id associated with this executor.
    #[must_use]
    pub const fn session_id(&self) -> &SessionId {
        &self.session_id
    }
}

#[async_trait]
impl AgentExecutor for SessionStoreExecutor {
    fn requires_durable_hitl_preparation(&self) -> bool {
        true
    }

    async fn checkpoint(
        &self,
        checkpoint: AgentCheckpoint,
    ) -> Result<AgentExecutionDecision, AgentExecutorError> {
        if let Some(lease) = self.admission_lease.as_ref() {
            self.store
                .commit_checkpoint_fenced(lease, checkpoint)
                .await?;
        } else {
            self.store
                .commit_checkpoint(&self.session_id, checkpoint)
                .await?;
        }
        Ok(AgentExecutionDecision::Continue)
    }
}
