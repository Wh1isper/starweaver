//! Runtime run result types and compatibility exports for checkpointable state.

use serde::{Deserialize, Serialize};

/// Checkpointable state shared with durability layers.
pub use starweaver_context::AgentRunState;
/// Shared lifecycle status for an admitted runtime execution.
pub use starweaver_core::RunLifecycle as RunStatus;

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
    pub fn pending_approvals(&self) -> &[starweaver_model::ToolReturnPart] {
        self.state.pending_approvals()
    }

    /// Return pending deferred tool returns.
    #[must_use]
    pub fn pending_deferred_tools(&self) -> &[starweaver_model::ToolReturnPart] {
        self.state.pending_deferred_tools()
    }
}
