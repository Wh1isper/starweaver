//! Tool trait, function-backed tools, and tool result values.

mod function;
mod result;
mod traits;
mod typed;

use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::{ToolContext, ToolError};

pub use function::FunctionTool;
pub use result::{DynTool, ToolResult};
pub use traits::{EmptyToolArgs, Tool};
pub use typed::TypedFunctionTool;

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
