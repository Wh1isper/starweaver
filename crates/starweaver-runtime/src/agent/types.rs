//! Agent runtime public types.

use serde::{Deserialize, Serialize};
use starweaver_model::{ModelError, ModelMessage};
use thiserror::Error;

use crate::{
    capability::CapabilityOrderError,
    executor::{AgentExecutionNode, AgentExecutorError},
    run::{AgentRunResult, AgentRunState},
    usage::UsageLimitError,
};

/// Runtime policy for bare agent runs.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentRuntimePolicy {
    /// Maximum model requests in one run.
    pub max_steps: usize,
    /// Maximum output validation retries.
    pub output_retries: usize,
}

impl Default for AgentRuntimePolicy {
    fn default() -> Self {
        Self {
            max_steps: 10_000,
            output_retries: 1,
        }
    }
}

/// Bare agent runtime error.
#[derive(Debug, Error)]
pub enum AgentError {
    /// Model adapter failed.
    #[error(transparent)]
    Model(#[from] ModelError),
    /// Capability hook failed.
    #[error("capability error: {0}")]
    Capability(String),
    /// Capability ordering failed.
    #[error(transparent)]
    CapabilityOrder(#[from] CapabilityOrderError),
    /// Structured output parsing failed.
    #[error("structured output error: {0}")]
    StructuredOutput(String),
    /// Dynamic instruction generation failed.
    #[error("dynamic instruction error: {0}")]
    DynamicInstruction(String),
    /// Output retry budget was exceeded.
    #[error("output retry limit exceeded after {retries} retries")]
    OutputRetryLimitExceeded {
        /// Retry count.
        retries: usize,
    },
    /// Tool retry budget was exceeded.
    #[error("tool {tool:?} exceeded max retries count of {max_retries}")]
    ToolRetryLimitExceeded {
        /// Tool name.
        tool: String,
        /// Retry limit for this tool.
        max_retries: usize,
    },
    /// Maximum step count was exceeded.
    #[error("step limit exceeded after {steps} steps")]
    StepLimitExceeded {
        /// Step count.
        steps: usize,
    },
    /// Usage limit was exceeded.
    #[error(transparent)]
    UsageLimit(#[from] UsageLimitError),
    /// Execution was suspended at a durable checkpoint.
    #[error("agent execution suspended at {node:?}: {reason}")]
    ExecutionSuspended {
        /// Suspended execution node.
        node: AgentExecutionNode,
        /// Suspend reason.
        reason: String,
    },
    /// Durable executor failed.
    #[error(transparent)]
    Executor(#[from] AgentExecutorError),
    /// Model returned tool calls before tool execution exists in this bare runtime.
    #[error("tool calls require starweaver-tools runtime support")]
    ToolCallsRequireTools,
}

/// Bare agent result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentResult {
    /// Final text output.
    pub output: String,
    /// Parsed structured output when an output schema is configured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_output: Option<serde_json::Value>,
    /// Canonical message history.
    pub messages: Vec<ModelMessage>,
    /// Final run state.
    pub state: AgentRunState,
    /// Number of messages supplied as prior history.
    pub history_len: usize,
}

impl AgentResult {
    /// Return all messages visible to the run.
    #[must_use]
    pub fn all_messages(&self) -> &[ModelMessage] {
        &self.messages
    }

    /// Return messages produced by this run.
    #[must_use]
    pub fn new_messages(&self) -> &[ModelMessage] {
        &self.messages[self.history_len..]
    }

    /// Parse structured output into a Rust type.
    ///
    /// # Errors
    ///
    /// Returns an error when no structured output is present or deserialization fails.
    pub fn structured<T>(&self) -> Result<T, AgentError>
    where
        T: serde::de::DeserializeOwned,
    {
        let value = self
            .structured_output
            .clone()
            .ok_or_else(|| AgentError::StructuredOutput("missing structured output".to_string()))?;
        serde_json::from_value(value)
            .map_err(|error| AgentError::StructuredOutput(error.to_string()))
    }
}

impl From<AgentRunResult> for AgentResult {
    fn from(result: AgentRunResult) -> Self {
        Self {
            output: result.output,
            structured_output: result.state.structured_output.clone(),
            messages: result.state.message_history.clone(),
            state: result.state,
            history_len: 0,
        }
    }
}
