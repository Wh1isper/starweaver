use std::{future::Future, sync::Arc};

use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use starweaver_core::Metadata;
use starweaver_tools::{
    DynTool, TOOL_METADATA_CONTEXT_MANAGEMENT_KEY, ToolContext, ToolError, ToolResult,
    typed_json_tool,
};

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
    Arc::new(typed_json_tool::<Args, _, _>(
        name,
        Some(description.to_string()),
        function,
    ))
}

pub fn static_sequential_tool<Args, F, Fut>(
    name: &'static str,
    description: &'static str,
    function: F,
) -> DynTool
where
    Args: DeserializeOwned + JsonSchema + Send + 'static,
    F: Send + Sync + 'static + Fn(ToolContext, Args) -> Fut,
    Fut: Send + Future<Output = Result<ToolResult, ToolError>> + 'static,
{
    Arc::new(
        typed_json_tool::<Args, _, _>(name, Some(description.to_string()), function)
            .with_sequential(true),
    )
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
        typed_json_tool::<Args, _, _>(name, Some(description.to_string()), function)
            .with_metadata(metadata),
    )
}

pub fn static_sequential_tool_with_metadata<Args, F, Fut>(
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
        typed_json_tool::<Args, _, _>(name, Some(description.to_string()), function)
            .with_metadata(metadata)
            .with_sequential(true),
    )
}

pub fn tool_metadata(bundle: &str, inherit: bool, approval_required: bool) -> Metadata {
    let mut metadata = Metadata::default();
    metadata.insert("bundle".to_string(), serde_json::json!(bundle));
    metadata.insert("auto_inherit".to_string(), serde_json::json!(inherit));
    if approval_required {
        metadata.insert("approval_required".to_string(), serde_json::json!(true));
    }
    metadata
}

pub fn context_management_tool_metadata(
    bundle: &str,
    inherit: bool,
    approval_required: bool,
) -> Metadata {
    let mut metadata = tool_metadata(bundle, inherit, approval_required);
    metadata.insert(
        TOOL_METADATA_CONTEXT_MANAGEMENT_KEY.to_string(),
        serde_json::json!(true),
    );
    metadata
}

pub fn tool_execution_error(tool: &str, error: impl std::fmt::Display) -> ToolError {
    ToolError::Execution {
        tool: tool.to_string(),
        message: error.to_string(),
    }
}

pub fn tool_invalid_arguments(tool: &str, message: impl Into<String>) -> ToolError {
    ToolError::InvalidArguments {
        tool: tool.to_string(),
        message: message.into(),
    }
}

pub fn tool_model_retry(tool: &str, message: impl Into<String>) -> ToolError {
    ToolError::ModelRetry {
        tool: tool.to_string(),
        message: message.into(),
    }
}
