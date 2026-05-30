use std::{future::Future, sync::Arc};

use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use starweaver_core::Metadata;
use starweaver_tools::{typed_tool, DynTool, ToolContext, ToolError, ToolResult};

pub fn static_tool<Args, F, Fut>(
    name: &'static str,
    description: &'static str,
    function: F,
) -> DynTool
where
    Args: DeserializeOwned + JsonSchema + Send + 'static,
    F: Send + Sync + 'static + Fn(ToolContext, Args) -> Fut,
    Fut: Send + Future<Output = Result<ToolResult, ToolError>> + 'static,
{
    Arc::new(typed_tool::<Args, _, _>(
        name,
        Some(description.to_string()),
        function,
    ))
}

pub fn static_tool_with_metadata<Args, F, Fut>(
    name: &'static str,
    description: &'static str,
    metadata: Metadata,
    function: F,
) -> DynTool
where
    Args: DeserializeOwned + JsonSchema + Send + 'static,
    F: Send + Sync + 'static + Fn(ToolContext, Args) -> Fut,
    Fut: Send + Future<Output = Result<ToolResult, ToolError>> + 'static,
{
    Arc::new(
        typed_tool::<Args, _, _>(name, Some(description.to_string()), function)
            .with_metadata(metadata),
    )
}

pub fn tool_execution_error(tool: &str, error: impl std::fmt::Display) -> ToolError {
    ToolError::Execution {
        tool: tool.to_string(),
        message: error.to_string(),
    }
}
