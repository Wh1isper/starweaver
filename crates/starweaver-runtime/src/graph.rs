//! Deterministic agent-loop graph transitions.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::run::{AgentRunState, RunStatus};

/// Agent runtime graph node.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentNode {
    /// Enter run and initialize state.
    StartRun,
    /// Build the next canonical request.
    PrepareRequest,
    /// Deliver queued steering messages.
    DrainMessages,
    /// Call model adapter.
    ModelRequest,
    /// Classify response as final output, tool calls, or retry.
    HandleResponse,
    /// Execute a batch of tool calls.
    ExecuteTools,
    /// Finalize output and export state.
    FinalizeRun,
    /// Drain idle messages after final output appears.
    DrainIdleMessages,
    /// Terminal node.
    Complete,
}

/// Pure graph transition decision.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GraphDecision {
    /// Next node to execute.
    pub next: AgentNode,
    /// Whether a checkpoint should be written at this boundary.
    pub checkpoint: bool,
}

impl GraphDecision {
    const fn checkpoint(next: AgentNode) -> Self {
        Self {
            next,
            checkpoint: true,
        }
    }

    const fn step(next: AgentNode) -> Self {
        Self {
            next,
            checkpoint: false,
        }
    }
}

/// Graph transition error.
#[derive(Debug, Error)]
pub enum GraphError {
    /// The current node cannot transition with the provided state.
    #[error("invalid graph state at {node:?}: {reason}")]
    InvalidState {
        /// Current node.
        node: AgentNode,
        /// Human-readable reason.
        reason: String,
    },
}

/// Determine the next graph node from current node and state.
///
/// # Errors
///
/// Returns an error when the current node requires state produced by an earlier handler.
pub fn next_node(
    current: AgentNode,
    state: &AgentRunState,
    max_steps: usize,
) -> Result<GraphDecision, GraphError> {
    if state.run_step >= max_steps
        && !matches!(
            current,
            AgentNode::FinalizeRun | AgentNode::DrainIdleMessages | AgentNode::Complete
        )
    {
        return Ok(GraphDecision::checkpoint(AgentNode::FinalizeRun));
    }

    match current {
        AgentNode::StartRun => Ok(GraphDecision::checkpoint(AgentNode::PrepareRequest)),
        AgentNode::PrepareRequest => Ok(GraphDecision::checkpoint(AgentNode::DrainMessages)),
        AgentNode::DrainMessages => Ok(GraphDecision::step(AgentNode::ModelRequest)),
        AgentNode::ModelRequest => {
            if state.latest_response.is_some() {
                Ok(GraphDecision::checkpoint(AgentNode::HandleResponse))
            } else {
                Err(GraphError::InvalidState {
                    node: current,
                    reason: "model response is required".to_string(),
                })
            }
        }
        AgentNode::HandleResponse => {
            if !state.pending_tool_calls.is_empty() {
                Ok(GraphDecision::checkpoint(AgentNode::ExecuteTools))
            } else if state.output.is_some() {
                Ok(GraphDecision::checkpoint(AgentNode::FinalizeRun))
            } else if !state.pending_tool_returns.is_empty() {
                Ok(GraphDecision::checkpoint(AgentNode::PrepareRequest))
            } else {
                Err(GraphError::InvalidState {
                    node: current,
                    reason: "tool calls, output, or retry signal is required".to_string(),
                })
            }
        }
        AgentNode::ExecuteTools => {
            if state.pending_tool_returns.is_empty() {
                Err(GraphError::InvalidState {
                    node: current,
                    reason: "tool returns are required after tool execution".to_string(),
                })
            } else {
                Ok(GraphDecision::checkpoint(AgentNode::PrepareRequest))
            }
        }
        AgentNode::FinalizeRun => {
            if matches!(
                state.status,
                RunStatus::Completed | RunStatus::Failed | RunStatus::Cancelled
            ) || state.output.is_some()
            {
                Ok(GraphDecision::checkpoint(AgentNode::DrainIdleMessages))
            } else {
                Err(GraphError::InvalidState {
                    node: current,
                    reason: "terminal output or terminal status is required".to_string(),
                })
            }
        }
        AgentNode::DrainIdleMessages => {
            if state.idle_messages.is_empty() {
                Ok(GraphDecision::checkpoint(AgentNode::Complete))
            } else {
                Ok(GraphDecision::checkpoint(AgentNode::PrepareRequest))
            }
        }
        AgentNode::Complete => Ok(GraphDecision::step(AgentNode::Complete)),
    }
}
