//! Session store trait, executor adapter, and built-in implementations.

mod contract;
mod memory;

use std::sync::Arc;

use async_trait::async_trait;
use starweaver_core::SessionId;
use starweaver_runtime::AgentCheckpoint;

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
        self.store
            .append_checkpoint(&self.session_id, checkpoint)
            .await?;
        Ok(starweaver_runtime::AgentExecutionDecision::Continue)
    }
}
