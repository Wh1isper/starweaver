use std::{future::Future, sync::Arc};

use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use starweaver_core::Metadata;
use starweaver_environment::EnvironmentError;
use starweaver_tools::{
    DynTool, TOOL_METADATA_DEPENDENCIES_KEY, ToolContext, ToolDependencyRequirements, ToolError,
    ToolResult, typed_json_tool,
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
    Arc::new(
        typed_json_tool::<Args, _, _>(name, Some(description.to_string()), function)
            .with_metadata(first_party_dependency_metadata()),
    )
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
            .with_metadata(first_party_dependency_metadata())
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
    let mut metadata = first_party_dependency_metadata();
    metadata.insert("bundle".to_string(), serde_json::json!(bundle));
    metadata.insert("auto_inherit".to_string(), serde_json::json!(inherit));
    if approval_required {
        metadata.insert("approval_required".to_string(), serde_json::json!(true));
    }
    metadata
}

pub fn tool_metadata_with_dependencies(
    bundle: &str,
    inherit: bool,
    approval_required: bool,
    requirements: &ToolDependencyRequirements,
) -> Metadata {
    let mut metadata = tool_metadata(bundle, inherit, approval_required);
    metadata.insert(
        TOOL_METADATA_DEPENDENCIES_KEY.to_string(),
        requirements.to_metadata_value(),
    );
    metadata
}

fn first_party_dependency_metadata() -> Metadata {
    let mut metadata = Metadata::default();
    metadata.insert(
        TOOL_METADATA_DEPENDENCIES_KEY.to_string(),
        ToolDependencyRequirements::filtered(Vec::<String>::new(), false).to_metadata_value(),
    );
    metadata
}

pub fn tool_execution_error(tool: &str, error: impl std::fmt::Display) -> ToolError {
    ToolError::Execution {
        tool: tool.to_string(),
        message: error.to_string(),
    }
}

pub fn tool_user_error(tool: &str, error: impl std::fmt::Display) -> ToolError {
    ToolError::UserError {
        tool: tool.to_string(),
        message: error.to_string(),
    }
}

pub fn tool_environment_error(tool: &str, error: EnvironmentError) -> ToolError {
    match error {
        EnvironmentError::Provider(message) => tool_execution_error(tool, message),
        EnvironmentError::NotFound(path) => tool_feedback(
            tool,
            format!(
                "environment resource not found: {path}. Verify the path or resource name, then retry with an existing target."
            ),
        ),
        EnvironmentError::AccessDenied(message) => tool_feedback(
            tool,
            format!(
                "environment access denied: {message}. Choose an allowed path, working directory, command, or resource according to the active environment policy."
            ),
        ),
        EnvironmentError::InvalidRequest(message) => tool_feedback(
            tool,
            format!("invalid environment request: {message}. Adjust the tool arguments and retry."),
        ),
        EnvironmentError::Unsupported(message) => tool_feedback(
            tool,
            format!(
                "unsupported environment operation: {message}. Use a provider that advertises this capability or choose a fallback workflow."
            ),
        ),
    }
}

pub fn tool_invalid_arguments(tool: &str, message: impl Into<String>) -> ToolError {
    tool_feedback(tool, message)
}

pub fn tool_feedback(tool: &str, message: impl Into<String>) -> ToolError {
    ToolError::Feedback {
        tool: tool.to_string(),
        message: message.into(),
    }
}
