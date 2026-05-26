//! Dynamic instructions for agent runs.

use async_trait::async_trait;
use thiserror::Error;

use crate::run::AgentRunState;

/// Shared dynamic instruction reference.
pub type DynDynamicInstruction = std::sync::Arc<dyn DynamicInstruction>;

/// Dynamic instruction generation error.
#[derive(Debug, Error)]
pub enum DynamicInstructionError {
    /// Dynamic instruction generation failed.
    #[error("dynamic instruction failed: {0}")]
    Failed(String),
}

impl DynamicInstructionError {
    /// Create a dynamic instruction failure.
    #[must_use]
    pub fn failed(message: impl Into<String>) -> Self {
        Self::Failed(message.into())
    }
}

/// Dynamic instruction result.
pub type DynamicInstructionResult<T> = Result<T, DynamicInstructionError>;

/// Instruction generator called per agent run.
#[async_trait]
pub trait DynamicInstruction: Send + Sync {
    /// Generate instruction text for the current run state.
    ///
    /// # Errors
    ///
    /// Returns an error when instruction generation fails.
    async fn instruction(&self, state: &AgentRunState) -> DynamicInstructionResult<String>;
}

/// Function-backed dynamic instruction.
pub struct FunctionDynamicInstruction<F> {
    function: F,
}

impl<F> FunctionDynamicInstruction<F> {
    /// Build a dynamic instruction from a function.
    #[must_use]
    pub const fn new(function: F) -> Self {
        Self { function }
    }
}

#[async_trait]
impl<F, Fut> DynamicInstruction for FunctionDynamicInstruction<F>
where
    F: Send + Sync + Fn(AgentRunState) -> Fut,
    Fut: Send + std::future::Future<Output = DynamicInstructionResult<String>>,
{
    async fn instruction(&self, state: &AgentRunState) -> DynamicInstructionResult<String> {
        (self.function)(state.clone()).await
    }
}
