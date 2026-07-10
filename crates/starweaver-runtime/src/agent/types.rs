//! Agent runtime public types.

use serde::{Deserialize, Serialize};
use starweaver_model::{ContentPart, ModelError, ModelMessage, ToolReturnPart};
use thiserror::Error;

use starweaver_usage::UsageLimitError;

use crate::{
    capability::CapabilityOrderError,
    executor::{AgentExecutionNode, AgentExecutorError},
    output::{OutputMedia, OutputValue},
    run::{AgentRunResult, AgentRunState},
};

/// User input for an agent run.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentInput {
    /// Ordered multimodal user content parts.
    pub content: Vec<ContentPart>,
}

impl AgentInput {
    /// Build input from ordered user content parts.
    #[must_use]
    pub fn new(content: impl Into<Vec<ContentPart>>) -> Self {
        Self {
            content: content.into(),
        }
    }

    /// Build text-only input.
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self::new(vec![ContentPart::text(text)])
    }

    /// Build input from ordered user content parts.
    #[must_use]
    pub fn parts(content: impl Into<Vec<ContentPart>>) -> Self {
        Self::new(content)
    }

    /// Return true when no content parts are present.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.content.is_empty()
    }

    pub(in crate::agent) fn text_projection(&self) -> String {
        self.content
            .iter()
            .filter_map(|part| match part {
                ContentPart::Text { text } => Some(text.as_str()),
                ContentPart::CachePoint { .. }
                | ContentPart::ImageUrl { .. }
                | ContentPart::FileUrl { .. }
                | ContentPart::Binary { .. }
                | ContentPart::ResourceRef { .. }
                | ContentPart::DataUrl { .. } => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl From<String> for AgentInput {
    fn from(text: String) -> Self {
        Self::text(text)
    }
}

impl From<&str> for AgentInput {
    fn from(text: &str) -> Self {
        Self::text(text)
    }
}

impl From<ContentPart> for AgentInput {
    fn from(content: ContentPart) -> Self {
        Self::new(vec![content])
    }
}

impl From<Vec<ContentPart>> for AgentInput {
    fn from(content: Vec<ContentPart>) -> Self {
        Self::new(content)
    }
}

/// Strategy for handling ordinary tool calls returned alongside a final output tool call.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentEndStrategy {
    /// Stop as soon as a valid output function returns final output.
    #[default]
    Early,
    /// Execute remaining ordinary tools, then complete with the first valid final output.
    Graceful,
    /// Execute all ordinary tools, then complete with the first valid final output.
    Exhaustive,
}

/// Runtime scheduling mode for a batch of model-returned tool calls.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentToolExecutionMode {
    /// Execute independent tool calls concurrently when no tool requests sequential execution.
    #[default]
    Parallel,
    /// Execute tool calls one at a time in model-returned order.
    Sequential,
}

/// Runtime policy for bare agent runs.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentRuntimePolicy {
    /// Maximum model requests in one run.
    pub max_steps: usize,
    /// Maximum output validation retries.
    pub output_retries: usize,
    /// How to handle ordinary tool calls returned alongside a final output function.
    #[serde(default)]
    pub end_strategy: AgentEndStrategy,
    /// How to schedule batches of model-returned tool calls.
    #[serde(default)]
    pub tool_execution: AgentToolExecutionMode,
}

impl Default for AgentRuntimePolicy {
    fn default() -> Self {
        Self {
            max_steps: 10_000,
            output_retries: 1,
            end_strategy: AgentEndStrategy::Early,
            tool_execution: AgentToolExecutionMode::Parallel,
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
    /// Runtime execution was cancelled cooperatively.
    #[error("agent run cancelled: {reason}")]
    Cancelled {
        /// Human-readable cancellation reason.
        reason: String,
    },
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

    /// Return media/file outputs from the latest model response.
    #[must_use]
    pub fn media_outputs(&self) -> Vec<OutputMedia> {
        self.messages
            .iter()
            .rev()
            .find_map(|message| match message {
                ModelMessage::Response(response) => Some(
                    response
                        .parts
                        .iter()
                        .filter_map(OutputMedia::from_response_part)
                        .collect::<Vec<_>>(),
                ),
                ModelMessage::Request(_) => None,
            })
            .unwrap_or_default()
    }

    /// Return image outputs from the latest model response.
    #[must_use]
    pub fn image_outputs(&self) -> Vec<OutputMedia> {
        self.media_outputs()
            .into_iter()
            .filter(OutputMedia::is_image)
            .collect()
    }

    /// Return the final output as text, JSON, or media wrappers.
    #[must_use]
    pub fn output_value(&self) -> OutputValue {
        let media = self.media_outputs();
        if !media.is_empty() {
            OutputValue::Media(media)
        } else if let Some(value) = self.structured_output.clone() {
            OutputValue::Json(value)
        } else {
            OutputValue::Text(self.output.clone())
        }
    }

    /// Return true when the run result is waiting for approval or deferred tool results.
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
