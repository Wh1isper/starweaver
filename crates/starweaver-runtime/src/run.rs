//! Runtime run state and result types.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use starweaver_core::{ConversationId, Metadata, RunId};
use starweaver_model::{ModelMessage, ModelResponse, ToolCallPart, ToolReturnPart};
use starweaver_usage::Usage;

/// Runtime status for an agent run.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    /// Run is being initialized.
    Starting,
    /// Run is actively executing graph nodes.
    Running,
    /// Run is waiting on external work.
    Waiting,
    /// Run completed successfully.
    Completed,
    /// Run failed.
    Failed,
    /// Run was cancelled.
    Cancelled,
}

/// Checkpointable state owned by the graph loop.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentRunState {
    /// Run identifier.
    pub run_id: RunId,
    /// Conversation identifier.
    pub conversation_id: ConversationId,
    /// Canonical model history.
    pub message_history: Vec<ModelMessage>,
    /// Accumulated usage.
    pub usage: Usage,
    /// Completed model/tool loop steps.
    pub run_step: usize,
    /// Current status.
    pub status: RunStatus,
    /// Final text output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// Final structured JSON output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_output: Option<Value>,
    /// Latest model response awaiting classification.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_response: Option<ModelResponse>,
    /// Tool calls awaiting execution.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_tool_calls: Vec<ToolCallPart>,
    /// Tool returns awaiting request preparation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_tool_returns: Vec<ToolReturnPart>,
    /// Tool calls that require approval before execution can proceed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_approval_tool_returns: Vec<ToolReturnPart>,
    /// Tool calls deferred to another runtime or durable worker.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deferred_tool_returns: Vec<ToolReturnPart>,
    /// Idle messages ready to redirect finalization.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub idle_messages: Vec<String>,
    /// Run metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

impl AgentRunState {
    /// Create empty run state.
    #[must_use]
    pub fn new(run_id: RunId, conversation_id: ConversationId) -> Self {
        Self {
            run_id,
            conversation_id,
            message_history: Vec::new(),
            usage: Usage::default(),
            run_step: 0,
            status: RunStatus::Starting,
            output: None,
            structured_output: None,
            latest_response: None,
            pending_tool_calls: Vec::new(),
            pending_tool_returns: Vec::new(),
            pending_approval_tool_returns: Vec::new(),
            deferred_tool_returns: Vec::new(),
            idle_messages: Vec::new(),
            metadata: Metadata::default(),
        }
    }

    /// Apply a model response to state.
    pub fn apply_model_response(&mut self, mut response: ModelResponse) {
        response.run_id.get_or_insert_with(|| self.run_id.clone());
        response
            .conversation_id
            .get_or_insert_with(|| self.conversation_id.clone());
        response.timestamp.get_or_insert_with(Utc::now);
        self.usage.add_assign(&response.usage);
        self.message_history
            .push(ModelMessage::Response(response.clone()));
        self.latest_response = Some(response);
    }

    /// Replace the latest response after lifecycle hooks mutate it.
    pub fn replace_latest_response(&mut self, response: ModelResponse) {
        if let Some(ModelMessage::Response(history_response)) = self.message_history.last_mut() {
            *history_response = response.clone();
        }
        self.latest_response = Some(response);
    }

    /// Return true when the run is waiting for approval or deferred tool results.
    #[must_use]
    pub const fn has_pending_hitl(&self) -> bool {
        !self.pending_approval_tool_returns.is_empty() || !self.deferred_tool_returns.is_empty()
    }

    /// Return pending approval-required tool returns.
    #[must_use]
    pub fn pending_approvals(&self) -> &[ToolReturnPart] {
        &self.pending_approval_tool_returns
    }

    /// Return pending deferred tool returns.
    #[must_use]
    pub fn pending_deferred_tools(&self) -> &[ToolReturnPart] {
        &self.deferred_tool_returns
    }

    /// Iterate all pending HITL tool returns in approval-then-deferred order.
    pub fn pending_hitl_tool_returns(&self) -> impl Iterator<Item = &ToolReturnPart> {
        self.pending_approval_tool_returns
            .iter()
            .chain(self.deferred_tool_returns.iter())
    }
}

/// Result returned when an agent run completes.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentRunResult {
    /// Final output text.
    pub output: String,
    /// Final checkpointable state.
    pub state: AgentRunState,
}

impl AgentRunResult {
    /// Return true when the final state is waiting for HITL input.
    #[must_use]
    pub const fn has_pending_hitl(&self) -> bool {
        self.state.has_pending_hitl()
    }

    /// Return pending approval-required tool returns.
    #[must_use]
    pub fn pending_approvals(&self) -> &[ToolReturnPart] {
        self.state.pending_approvals()
    }

    /// Return pending deferred tool returns.
    #[must_use]
    pub fn pending_deferred_tools(&self) -> &[ToolReturnPart] {
        self.state.pending_deferred_tools()
    }
}
