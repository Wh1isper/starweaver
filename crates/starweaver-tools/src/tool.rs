//! Tool trait, function-backed tools, and tool result values.

mod function;
mod result;
mod typed;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use starweaver_core::Metadata;
use starweaver_model::ToolDefinition;

use crate::{ToolContext, ToolError};

pub use function::FunctionTool;
pub use result::{DynTool, ToolResult};
pub use typed::TypedFunctionTool;

/// Empty object arguments for tools without input fields.
#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub struct EmptyToolArgs {}

/// Provider-neutral function tool trait.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Tool name exposed to the model.
    fn name(&self) -> &str;

    /// Tool description.
    fn description(&self) -> Option<&str>;

    /// JSON schema for tool arguments.
    fn parameters_schema(&self) -> Value;

    /// Runtime metadata attached to this tool definition.
    fn metadata(&self) -> Metadata {
        Metadata::default()
    }

    /// Per-tool retry override.
    fn max_retries(&self) -> Option<usize> {
        None
    }

    /// Execute a tool call.
    ///
    /// # Errors
    ///
    /// Returns an error when validation, approval, deferral, or execution fails.
    async fn call(&self, context: ToolContext, arguments: Value) -> Result<ToolResult, ToolError>;

    /// Convert this tool into a provider-neutral model definition.
    fn definition(&self) -> ToolDefinition {
        let mut metadata = self.metadata();
        if let Some(max_retries) = self.max_retries() {
            metadata.insert("max_retries".to_string(), serde_json::json!(max_retries));
        }
        ToolDefinition {
            name: self.name().to_string(),
            description: self.description().map(str::to_string),
            parameters: self.parameters_schema(),
            metadata,
        }
    }
}

/// Create a JSON-returning tool from an async function over raw JSON arguments.
#[must_use]
pub fn json_tool<F, Fut>(
    name: impl Into<String>,
    description: impl Into<Option<String>>,
    parameters: Value,
    function: F,
) -> FunctionTool<impl Send + Sync + Fn(ToolContext, Value) -> Fut>
where
    F: Send + Sync + Fn(ToolContext, Value) -> Fut,
    Fut: Send + std::future::Future<Output = Result<ToolResult, ToolError>>,
{
    FunctionTool::new(name, description, parameters, function)
}

/// Create a JSON-returning tool from an async function over typed arguments.
#[must_use]
pub fn typed_json_tool<Args, F, Fut>(
    name: impl Into<String>,
    description: impl Into<Option<String>>,
    function: F,
) -> TypedFunctionTool<Args, impl Send + Sync + Fn(ToolContext, Args) -> Fut>
where
    Args: DeserializeOwned + JsonSchema + Send + 'static,
    F: Send + Sync + Fn(ToolContext, Args) -> Fut,
    Fut: Send + std::future::Future<Output = Result<ToolResult, ToolError>>,
{
    TypedFunctionTool::new(name, description, function)
}
