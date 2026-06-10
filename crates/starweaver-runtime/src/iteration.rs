//! Run iteration inspection records derived from typed stream events.

use serde::{Deserialize, Serialize};

use crate::{
    executor::AgentExecutionNode,
    stream::{AgentStreamEvent, AgentStreamRecord},
    AgentResult,
};

/// Coarse iteration event kind for run inspection.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentIterationKind {
    /// A run started.
    RunStart,
    /// Runtime execution entered a durable node boundary.
    NodeStart,
    /// Runtime execution completed a durable node boundary.
    NodeComplete,
    /// A context sideband event was published.
    Custom,
    /// A model request was prepared.
    ModelRequest,
    /// A provider stream delta or part event was observed.
    ModelStream,
    /// A final model response was applied.
    ModelResponse,
    /// A durable checkpoint was observed.
    Checkpoint,
    /// Execution suspended at a checkpoint.
    Suspended,
    /// A tool call was observed.
    ToolCall,
    /// A tool return was observed.
    ToolReturn,
    /// Output validation requested another model turn.
    OutputRetry,
    /// Pending steering requested another model turn.
    SteeringGuard,
    /// The run completed.
    RunComplete,
    /// The run failed after preserving recoverable context state.
    RunFailed,
}

/// One inspected runtime iteration step.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentIterationStep {
    /// Monotonic iteration index.
    pub index: usize,
    /// Source stream record sequence.
    pub stream_sequence: usize,
    /// Current model/tool loop step when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_step: Option<usize>,
    /// Iteration kind.
    pub kind: AgentIterationKind,
    /// Execution node when the event is tied to a durable boundary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<AgentExecutionNode>,
}

impl AgentIterationStep {
    const fn from_record(index: usize, record: &AgentStreamRecord) -> Self {
        let (kind, run_step, node) = match &record.event {
            AgentStreamEvent::RunStart { .. } => (AgentIterationKind::RunStart, None, None),
            AgentStreamEvent::NodeStart { node, step, .. } => {
                (AgentIterationKind::NodeStart, Some(*step), Some(*node))
            }
            AgentStreamEvent::NodeComplete { node, step, .. } => {
                (AgentIterationKind::NodeComplete, Some(*step), Some(*node))
            }
            AgentStreamEvent::Custom { .. } => (AgentIterationKind::Custom, None, None),
            AgentStreamEvent::ModelRequest { step } => {
                (AgentIterationKind::ModelRequest, Some(*step), None)
            }
            AgentStreamEvent::ModelStream { step, .. } => {
                (AgentIterationKind::ModelStream, Some(*step), None)
            }
            AgentStreamEvent::ModelResponse { step, .. } => {
                (AgentIterationKind::ModelResponse, Some(*step), None)
            }
            AgentStreamEvent::Checkpoint { node, step } => {
                (AgentIterationKind::Checkpoint, Some(*step), Some(*node))
            }
            AgentStreamEvent::Suspended { node, .. } => {
                (AgentIterationKind::Suspended, None, Some(*node))
            }
            AgentStreamEvent::ToolCall { step, .. } => {
                (AgentIterationKind::ToolCall, Some(*step), None)
            }
            AgentStreamEvent::ToolReturn { step, .. } => {
                (AgentIterationKind::ToolReturn, Some(*step), None)
            }
            AgentStreamEvent::OutputRetry { .. } => (AgentIterationKind::OutputRetry, None, None),
            AgentStreamEvent::SteeringGuard { step, .. } => {
                (AgentIterationKind::SteeringGuard, Some(*step), None)
            }
            AgentStreamEvent::RunComplete { .. } => (AgentIterationKind::RunComplete, None, None),
            AgentStreamEvent::RunFailed { .. } => (AgentIterationKind::RunFailed, None, None),
        };
        Self {
            index,
            stream_sequence: record.sequence,
            run_step,
            kind,
            node,
        }
    }
}

/// Compact run iteration trace for debuggers, CLIs, and durable service UIs.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentIterationTrace {
    /// Iteration steps derived from stream records.
    pub steps: Vec<AgentIterationStep>,
}

impl AgentIterationTrace {
    /// Build an iteration trace from recorded stream events.
    #[must_use]
    pub fn from_stream_records(records: &[AgentStreamRecord]) -> Self {
        Self {
            steps: records
                .iter()
                .enumerate()
                .map(|(index, record)| AgentIterationStep::from_record(index, record))
                .collect(),
        }
    }

    /// Return all iteration steps.
    #[must_use]
    pub fn steps(&self) -> &[AgentIterationStep] {
        &self.steps
    }

    /// Return whether the trace includes a completed run.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.steps
            .iter()
            .any(|step| step.kind == AgentIterationKind::RunComplete)
    }
}

/// Result returned by collection-based iteration inspection runs.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentIterResult {
    /// Final agent result.
    pub result: AgentResult,
    /// Compact iteration trace.
    pub iterations: AgentIterationTrace,
    /// Source stream records used to build the trace.
    pub events: Vec<AgentStreamRecord>,
}

impl AgentIterResult {
    /// Return the final agent result.
    #[must_use]
    pub const fn result(&self) -> &AgentResult {
        &self.result
    }

    /// Return the iteration trace.
    #[must_use]
    pub const fn iterations(&self) -> &AgentIterationTrace {
        &self.iterations
    }

    /// Return the source stream records.
    #[must_use]
    pub fn events(&self) -> &[AgentStreamRecord] {
        &self.events
    }
}
