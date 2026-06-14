use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use starweaver_model::ToolDefinition;

use crate::run::AgentRunState;

use super::{OutputValidationResult, OutputValue};

/// Function-style final output call definition.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OutputFunctionDefinition {
    /// Output function name exposed to the model.
    pub name: String,
    /// Output function description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON schema for output function arguments.
    pub parameters: Value,
}

impl OutputFunctionDefinition {
    /// Build an output function definition.
    #[must_use]
    pub fn new(name: impl Into<String>, parameters: Value) -> Self {
        Self {
            name: name.into(),
            description: None,
            parameters,
        }
    }

    /// Add description.
    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Convert to a provider-neutral tool definition.
    #[must_use]
    pub fn tool_definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name.clone(),
            description: self.description.clone(),
            parameters: self.parameters.clone(),
            metadata: serde_json::Map::new(),
        }
    }
}

/// Output function execution context.
#[derive(Clone, Debug)]
pub struct OutputFunctionContext {
    /// Run state at finalization time.
    pub state: AgentRunState,
}

/// Output function called by the model to finish a run.
#[async_trait]
pub trait OutputFunction: Send + Sync {
    /// Output function definition exposed to the model.
    fn definition(&self) -> OutputFunctionDefinition;

    /// Execute the output function with model-provided arguments.
    ///
    /// # Errors
    ///
    /// Returns an error when the output function rejects the arguments or fails.
    async fn call(
        &self,
        context: OutputFunctionContext,
        arguments: Value,
    ) -> OutputValidationResult<OutputValue>;
}

/// Function-backed output function.
pub struct FunctionOutputFunction<F> {
    definition: OutputFunctionDefinition,
    function: F,
}

impl<F> FunctionOutputFunction<F> {
    /// Build a function-backed output function.
    #[must_use]
    pub const fn new(definition: OutputFunctionDefinition, function: F) -> Self {
        Self {
            definition,
            function,
        }
    }
}

#[async_trait]
impl<F, Fut> OutputFunction for FunctionOutputFunction<F>
where
    F: Send + Sync + Fn(OutputFunctionContext, Value) -> Fut,
    Fut: Send + std::future::Future<Output = OutputValidationResult<OutputValue>>,
{
    fn definition(&self) -> OutputFunctionDefinition {
        self.definition.clone()
    }

    async fn call(
        &self,
        context: OutputFunctionContext,
        arguments: Value,
    ) -> OutputValidationResult<OutputValue> {
        (self.function)(context, arguments).await
    }
}
