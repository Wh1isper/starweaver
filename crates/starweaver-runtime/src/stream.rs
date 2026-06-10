//! Typed agent stream event foundations.

use serde::{Deserialize, Serialize};
use starweaver_context::AgentEvent;
use starweaver_core::{ConversationId, RunId};
use starweaver_model::{ModelResponse, ModelResponseStreamEvent, ToolCallPart, ToolReturnPart};

use crate::{executor::AgentExecutionNode, run::RunStatus, AgentResult};

/// Typed event emitted by the agent runtime while a run progresses.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentStreamEvent {
    /// A run started.
    RunStart {
        /// Run identifier.
        run_id: RunId,
        /// Conversation identifier.
        conversation_id: ConversationId,
    },
    /// Runtime execution entered a durable node boundary.
    NodeStart {
        /// Execution boundary being entered.
        node: AgentExecutionNode,
        /// Completed run step at this boundary.
        step: usize,
        /// Current run status at this boundary.
        status: RunStatus,
    },
    /// Runtime execution completed a durable node boundary.
    NodeComplete {
        /// Execution boundary that completed.
        node: AgentExecutionNode,
        /// Completed run step at this boundary.
        step: usize,
        /// Current run status after this boundary.
        status: RunStatus,
    },
    /// A context sideband event was published during the run.
    Custom {
        /// Application or capability event.
        event: AgentEvent,
    },
    /// A model request was prepared for a loop step.
    ModelRequest {
        /// Completed run step before sending the request.
        step: usize,
    },
    /// A model response stream event was received.
    ModelStream {
        /// Completed run step for the active model request.
        step: usize,
        /// Canonical model stream event.
        event: ModelResponseStreamEvent,
    },
    /// A model response was received.
    ModelResponse {
        /// Completed run step after receiving the response.
        step: usize,
        /// Canonical model response.
        response: ModelResponse,
    },
    /// A durable execution checkpoint was persisted or inspected.
    Checkpoint {
        /// Execution boundary.
        node: AgentExecutionNode,
        /// Completed run step at this boundary.
        step: usize,
    },
    /// Execution was suspended at a durable checkpoint.
    Suspended {
        /// Execution boundary that requested suspension.
        node: AgentExecutionNode,
        /// Human-readable suspend reason.
        reason: String,
    },
    /// A model requested a function tool call.
    ToolCall {
        /// Current run step.
        step: usize,
        /// Tool call part.
        call: ToolCallPart,
    },
    /// A function tool returned a result or structured control-flow error.
    ToolReturn {
        /// Current run step.
        step: usize,
        /// Tool return part.
        tool_return: ToolReturnPart,
    },
    /// Output validation or output function validation requested another model turn.
    OutputRetry {
        /// Retry count after this retry was scheduled.
        retries: usize,
        /// Retry prompt sent to the model.
        prompt: String,
    },
    /// Pending user steering requested another model turn before finalization.
    SteeringGuard {
        /// Current run step.
        step: usize,
        /// Control prompt sent to the model before finalization.
        prompt: String,
    },
    /// A run completed successfully.
    RunComplete {
        /// Run identifier.
        run_id: RunId,
        /// Final output text.
        output: String,
    },
    /// A run failed after preserving recoverable context state.
    RunFailed {
        /// Run identifier.
        run_id: RunId,
        /// Failure kind.
        error_kind: String,
        /// Human-readable error message.
        message: String,
    },
}

/// Sequenced stream event record.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentStreamRecord {
    /// Monotonic event sequence number within one run.
    pub sequence: usize,
    /// Typed event payload.
    pub event: AgentStreamEvent,
}

impl AgentStreamRecord {
    /// Create a sequenced stream record.
    #[must_use]
    pub const fn new(sequence: usize, event: AgentStreamEvent) -> Self {
        Self { sequence, event }
    }
}

/// Result returned by collection-based stream runs.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentStreamResult {
    /// Final agent result.
    pub result: AgentResult,
    /// Events captured while the run progressed.
    pub events: Vec<AgentStreamRecord>,
}

impl AgentStreamResult {
    /// Return captured stream records.
    #[must_use]
    pub fn events(&self) -> &[AgentStreamRecord] {
        &self.events
    }

    /// Return the final result.
    #[must_use]
    pub const fn result(&self) -> &AgentResult {
        &self.result
    }
}

pub(crate) fn push_stream_event(
    events: &mut Option<&mut Vec<AgentStreamRecord>>,
    event: AgentStreamEvent,
) {
    if let Some(events) = events.as_deref_mut() {
        events.push(AgentStreamRecord::new(events.len(), event));
    }
}
