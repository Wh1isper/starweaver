//! Runtime checkpoint emission helpers and compatibility exports.

use std::sync::Arc;

use async_trait::async_trait;
pub use starweaver_context::{
    AgentCheckpoint, AgentExecutionDecision, AgentExecutor, AgentExecutorError, AgentResumeCursor,
    AgentResumeEvidence,
};
pub use starweaver_core::AgentExecutionNode;

/// Shared executor reference.
pub type DynAgentExecutor = Arc<dyn AgentExecutor>;

/// Direct in-process executor that always continues.
#[derive(Clone, Debug, Default)]
pub struct DirectAgentExecutor;

#[async_trait]
impl AgentExecutor for DirectAgentExecutor {
    async fn checkpoint(
        &self,
        _checkpoint: AgentCheckpoint,
    ) -> Result<AgentExecutionDecision, AgentExecutorError> {
        Ok(AgentExecutionDecision::Continue)
    }
}
