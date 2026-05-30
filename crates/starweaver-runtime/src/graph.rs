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

/// One inspected graph transition.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentGraphStep {
    /// Step index within the inspected graph walk.
    pub index: usize,
    /// Node inspected for this transition.
    pub current: AgentNode,
    /// Transition decision returned by the graph.
    pub decision: GraphDecision,
    /// Completed runtime step from the inspected state.
    pub run_step: usize,
    /// Runtime status from the inspected state.
    pub status: RunStatus,
}

/// A compact graph inspection report.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentGraphTrace {
    /// Transition steps in walk order.
    pub steps: Vec<AgentGraphStep>,
    /// Last node reached by the inspection.
    pub terminal: AgentNode,
}

impl AgentGraphTrace {
    /// Return all inspected steps.
    #[must_use]
    pub fn steps(&self) -> &[AgentGraphStep] {
        &self.steps
    }

    /// Return whether the trace reached the complete node.
    #[must_use]
    pub const fn is_complete(&self) -> bool {
        matches!(self.terminal, AgentNode::Complete)
    }
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

/// Inspect the next graph transition from a node and state.
///
/// # Errors
///
/// Returns an error when the current node requires state produced by an earlier handler.
pub fn inspect_next_node(
    current: AgentNode,
    state: &AgentRunState,
    max_steps: usize,
) -> Result<AgentGraphStep, GraphError> {
    let decision = next_node(current, state, max_steps)?;
    Ok(AgentGraphStep {
        index: 0,
        current,
        decision,
        run_step: state.run_step,
        status: state.status,
    })
}

/// Walk graph transitions for inspection using a static state snapshot.
///
/// This is intended for application debuggers and durable runtime diagnostics. Runtime handlers mutate
/// state between nodes during a real run, so this helper stops at the first transition that needs state
/// produced by an earlier handler.
///
/// # Errors
///
/// Returns an error when the inspected transition is invalid for the provided state snapshot.
pub fn inspect_graph(
    start: AgentNode,
    state: &AgentRunState,
    max_steps: usize,
    max_transitions: usize,
) -> Result<AgentGraphTrace, GraphError> {
    let mut current = start;
    let mut steps = Vec::new();
    for index in 0..max_transitions {
        let decision = next_node(current, state, max_steps)?;
        let next = decision.next;
        steps.push(AgentGraphStep {
            index,
            current,
            decision,
            run_step: state.run_step,
            status: state.status,
        });
        current = next;
        if matches!(current, AgentNode::Complete) {
            break;
        }
    }
    Ok(AgentGraphTrace {
        steps,
        terminal: current,
    })
}
