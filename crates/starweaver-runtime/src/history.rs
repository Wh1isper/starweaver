//! Message history processors for agent runs.

use async_trait::async_trait;
use starweaver_model::{ModelMessage, ModelRequest, ModelRequestPart};
use thiserror::Error;

use crate::run::AgentRunState;

/// Message history processor error.
#[derive(Debug, Error)]
pub enum HistoryProcessorError {
    /// Processor failed.
    #[error("history processor failed: {0}")]
    Failed(String),
}

impl HistoryProcessorError {
    /// Create a processor failure.
    #[must_use]
    pub fn failed(message: impl Into<String>) -> Self {
        Self::Failed(message.into())
    }
}

/// History processor result.
pub type HistoryProcessorResult<T> = Result<T, HistoryProcessorError>;

/// Processor that can compact, filter, or reinject model history before a model request.
#[async_trait]
pub trait HistoryProcessor: Send + Sync {
    /// Process the request history sent to the model.
    ///
    /// # Errors
    ///
    /// Returns an error when history processing fails.
    async fn process(
        &self,
        state: &AgentRunState,
        messages: Vec<ModelMessage>,
    ) -> HistoryProcessorResult<Vec<ModelMessage>>;
}

/// Function-backed history processor.
pub struct FunctionHistoryProcessor<F> {
    function: F,
}

impl<F> FunctionHistoryProcessor<F> {
    /// Build a processor from a function.
    #[must_use]
    pub const fn new(function: F) -> Self {
        Self { function }
    }
}

#[async_trait]
impl<F, Fut> HistoryProcessor for FunctionHistoryProcessor<F>
where
    F: Send + Sync + Fn(Vec<ModelMessage>) -> Fut,
    Fut: Send + std::future::Future<Output = HistoryProcessorResult<Vec<ModelMessage>>>,
{
    async fn process(
        &self,
        _state: &AgentRunState,
        messages: Vec<ModelMessage>,
    ) -> HistoryProcessorResult<Vec<ModelMessage>> {
        (self.function)(messages).await
    }
}

/// History processor that preserves system prompts and structured instructions after filtering or compaction.
#[derive(Clone, Copy, Debug, Default)]
pub struct ReinjectSystemPromptProcessor;

impl ReinjectSystemPromptProcessor {
    /// Create a reinjection processor.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

#[async_trait]
impl HistoryProcessor for ReinjectSystemPromptProcessor {
    async fn process(
        &self,
        state: &AgentRunState,
        mut messages: Vec<ModelMessage>,
    ) -> HistoryProcessorResult<Vec<ModelMessage>> {
        let source_parts = instruction_parts(&state.message_history);
        if source_parts.is_empty() || has_all_instruction_parts(&messages, &source_parts) {
            return Ok(messages);
        }

        let request = ModelRequest {
            parts: source_parts,
            timestamp: None,
            instructions: None,
            run_id: Some(state.run_id.clone()),
            conversation_id: Some(state.conversation_id.clone()),
            metadata: serde_json::Map::new(),
        };
        messages.insert(0, ModelMessage::Request(request));
        Ok(messages)
    }
}

fn instruction_parts(messages: &[ModelMessage]) -> Vec<ModelRequestPart> {
    messages
        .iter()
        .flat_map(|message| match message {
            ModelMessage::Request(request) => request
                .parts
                .iter()
                .filter(|part| {
                    matches!(
                        part,
                        ModelRequestPart::SystemPrompt { .. }
                            | ModelRequestPart::Instruction { .. }
                    )
                })
                .cloned()
                .collect::<Vec<_>>(),
            ModelMessage::Response(_) => Vec::new(),
        })
        .collect()
}

fn has_all_instruction_parts(messages: &[ModelMessage], expected: &[ModelRequestPart]) -> bool {
    let existing = instruction_parts(messages);
    expected.iter().all(|part| existing.contains(part))
}
