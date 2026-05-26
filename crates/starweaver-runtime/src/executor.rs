//! Durable execution checkpoints for agent runs.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use starweaver_core::{ConversationId, Metadata, RunId};
use thiserror::Error;

use crate::run::AgentRunState;

/// Named execution boundary in the agent loop.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentExecutionNode {
    /// Run initialization completed.
    RunStart,
    /// The next model request is about to be prepared.
    PrepareModelRequest,
    /// The model request is about to be sent or skipped by policy.
    BeforeModelRequest,
    /// A model response has been applied to state.
    ModelResponse,
    /// A function tool call is about to execute.
    ToolCall,
    /// A function tool result has been applied to state.
    ToolReturn,
    /// Final output validation is about to run.
    ValidateOutput,
    /// The run completed.
    RunComplete,
    /// The run failed.
    RunFailed,
}

/// Serializable checkpoint emitted at a durable execution boundary.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentCheckpoint {
    /// Run identifier.
    pub run_id: RunId,
    /// Conversation identifier.
    pub conversation_id: ConversationId,
    /// Execution boundary.
    pub node: AgentExecutionNode,
    /// Completed run step at this boundary.
    pub run_step: usize,
    /// Full checkpointable run state.
    pub state: AgentRunState,
    /// Boundary metadata for node-specific details.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

impl AgentCheckpoint {
    /// Build a checkpoint from run state.
    #[must_use]
    pub fn new(node: AgentExecutionNode, state: &AgentRunState) -> Self {
        Self {
            run_id: state.run_id.clone(),
            conversation_id: state.conversation_id.clone(),
            node,
            run_step: state.run_step,
            state: state.clone(),
            metadata: Metadata::default(),
        }
    }

    /// Attach checkpoint metadata.
    #[must_use]
    pub fn with_metadata(mut self, metadata: Metadata) -> Self {
        self.metadata = metadata;
        self
    }
}

/// Decision returned by an execution checkpoint handler.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentExecutionDecision {
    /// Continue executing the run.
    Continue,
    /// Suspend execution after persisting the checkpoint.
    Suspend {
        /// Human-readable suspend reason.
        reason: String,
    },
}

/// Executor failure.
#[derive(Debug, Error)]
pub enum AgentExecutorError {
    /// Executor storage or policy failed.
    #[error("executor failed: {0}")]
    Failed(String),
}

/// Fine-grained executor hook for persistence, interruption, and durable scheduling.
#[async_trait]
pub trait AgentExecutor: Send + Sync {
    /// Persist or inspect a checkpoint and decide whether execution should continue.
    async fn checkpoint(
        &self,
        checkpoint: AgentCheckpoint,
    ) -> Result<AgentExecutionDecision, AgentExecutorError>;
}

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
