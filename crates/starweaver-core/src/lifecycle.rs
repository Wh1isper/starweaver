//! Shared run lifecycle vocabulary.

use serde::{Deserialize, Serialize};

/// Named execution boundary in the agent loop and durable stream protocol.
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

/// Lifecycle of an admitted runtime execution.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunLifecycle {
    /// Runtime initialization is in progress.
    Starting,
    /// Runtime is actively executing.
    Running,
    /// Runtime is waiting for external work or a decision.
    Waiting,
    /// Runtime completed successfully.
    Completed,
    /// Runtime failed.
    Failed,
    /// Runtime was cancelled or interrupted.
    Cancelled,
}

impl RunLifecycle {
    /// Return the stable wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Waiting => "waiting",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    /// Return whether this lifecycle is terminal.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}
