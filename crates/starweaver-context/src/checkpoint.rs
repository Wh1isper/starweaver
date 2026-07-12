//! Durable checkpoint records and executor callback contracts.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use starweaver_core::{
    AgentExecutionNode, CheckpointId, ConversationId, Metadata, RunId, RunLifecycle as RunStatus,
    TraceContext,
};
use starweaver_usage::Usage;
use thiserror::Error;

use crate::AgentRunState;

/// Compact cursor values durable stores use to resume and audit a run.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentResumeCursor {
    /// Index of the next model request attempt.
    pub model_request_attempt: usize,
    /// Current tool call batch identifier when the run is inside a tool boundary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_batch_id: Option<String>,
    /// Index of the next output validation attempt.
    pub output_validation_attempt: usize,
    /// Cursor of the last persisted stream event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_cursor: Option<usize>,
    /// Last canonical message index persisted by the runtime.
    pub message_cursor: usize,
}

/// Stable resume evidence for durable service runtimes and external session stores.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentResumeEvidence {
    /// Execution boundary captured by this checkpoint.
    pub node: AgentExecutionNode,
    /// Current run status.
    pub status: RunStatus,
    /// Completed run step at this boundary.
    pub run_step: usize,
    /// Resume cursors for replay and continuation.
    pub cursor: AgentResumeCursor,
    /// Accumulated usage snapshot.
    pub usage: Usage,
    /// Context state revision or hash provided by a service layer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_revision: Option<String>,
    /// Environment provider state reference provided by a service layer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_ref: Option<String>,
    /// Pending approval count.
    pub pending_approval_count: usize,
    /// Deferred tool return count.
    pub deferred_tool_count: usize,
    /// Trace correlation snapshot.
    #[serde(default, skip_serializing_if = "TraceContext::is_empty")]
    pub trace_context: TraceContext,
    /// Resume metadata for session store implementations.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

impl AgentResumeEvidence {
    /// Build resume evidence from run state and boundary metadata.
    #[must_use]
    pub fn new(node: AgentExecutionNode, state: &AgentRunState) -> Self {
        Self {
            node,
            status: state.status,
            run_step: state.run_step,
            cursor: AgentResumeCursor {
                model_request_attempt: state.run_step,
                tool_call_batch_id: (!state.pending_tool_calls.is_empty())
                    .then(|| format!("tool_batch_{}", state.run_step)),
                output_validation_attempt: 0,
                stream_cursor: None,
                message_cursor: state.message_history.len(),
            },
            usage: state.usage.clone(),
            context_revision: state
                .metadata
                .get("context_revision")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            environment_ref: state
                .metadata
                .get("environment_ref")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            pending_approval_count: state.pending_approval_tool_returns.len(),
            deferred_tool_count: state.deferred_tool_returns.len(),
            trace_context: TraceContext::default(),
            metadata: Metadata::default(),
        }
    }

    /// Attach stream cursor.
    #[must_use]
    pub const fn with_stream_cursor(mut self, stream_cursor: usize) -> Self {
        self.cursor.stream_cursor = Some(stream_cursor);
        self
    }

    /// Attach trace context.
    #[must_use]
    pub fn with_trace_context(mut self, trace_context: TraceContext) -> Self {
        self.trace_context = trace_context;
        self
    }

    /// Attach evidence metadata.
    #[must_use]
    pub fn with_metadata(mut self, metadata: Metadata) -> Self {
        self.metadata = metadata;
        self
    }
}

/// Serializable checkpoint emitted at a durable execution boundary.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentCheckpoint {
    /// Checkpoint identifier.
    pub checkpoint_id: CheckpointId,
    /// Run identifier.
    pub run_id: RunId,
    /// Conversation identifier.
    pub conversation_id: ConversationId,
    /// Execution boundary.
    pub node: AgentExecutionNode,
    /// Completed run step at this boundary.
    pub run_step: usize,
    /// Stable resume evidence for durable services.
    pub resume: AgentResumeEvidence,
    /// Full checkpointable run state.
    pub state: AgentRunState,
    /// Boundary metadata for node-specific details.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

impl starweaver_core::VersionedRecord for AgentCheckpoint {
    const SCHEMA: &'static str = "starweaver.runtime.checkpoint";
    const ALLOW_BARE_V0: bool = true;
}

impl AgentCheckpoint {
    /// Build a checkpoint from run state.
    #[must_use]
    pub fn new(node: AgentExecutionNode, state: &AgentRunState) -> Self {
        Self {
            checkpoint_id: CheckpointId::new(),
            run_id: state.run_id.clone(),
            conversation_id: state.conversation_id.clone(),
            node,
            run_step: state.run_step,
            resume: AgentResumeEvidence::new(node, state),
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

    /// Attach the last persisted stream cursor.
    #[must_use]
    pub fn with_stream_cursor(mut self, stream_cursor: usize) -> Self {
        self.resume = self.resume.with_stream_cursor(stream_cursor);
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

/// Callback contract for persistence, interruption, and durable scheduling.
#[async_trait]
pub trait AgentExecutor: Send + Sync {
    /// Persist or inspect a checkpoint and decide whether execution should continue.
    async fn checkpoint(
        &self,
        checkpoint: AgentCheckpoint,
    ) -> Result<AgentExecutionDecision, AgentExecutorError>;
}
