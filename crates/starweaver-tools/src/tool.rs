//! Tool trait, function-backed tools, and tool result values.

use std::{marker::PhantomData, sync::Arc};

use async_trait::async_trait;
use schemars::{schema_for, JsonSchema};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use starweaver_core::Metadata;
use starweaver_model::ToolDefinition;

use crate::{ToolContext, ToolError};

/// Shared reference to a runtime tool.
pub type DynTool = Arc<dyn Tool>;

/// Result returned by a function tool.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolResult {
    /// JSON-serializable tool content.
    pub content: Value,
    /// Tool result metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

impl ToolResult {
    /// Create a result from JSON content.
    #[must_use]
    pub fn new(content: Value) -> Self {
        Self {
            content,
            metadata: Metadata::default(),
        }
    }
}

impl From<Value> for ToolResult {
    fn from(value: Value) -> Self {
        Self::new(value)
    }
}

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

/// Function-backed tool with a caller-provided JSON schema.
pub struct FunctionTool<F> {
    name: String,
    description: Option<String>,
    parameters: Value,
    metadata: Metadata,
    max_retries: Option<usize>,
    function: F,
}

impl<F> FunctionTool<F> {
    /// Build a function-backed tool.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        description: impl Into<Option<String>>,
        parameters: Value,
        function: F,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
            metadata: Metadata::default(),
            max_retries: None,
            function,
        }
    }

    /// Attach runtime metadata to this tool.
    #[must_use]
    pub fn with_metadata(mut self, metadata: Metadata) -> Self {
        self.metadata = metadata;
        self
    }

    /// Override the retry budget for this tool.
    #[must_use]
    pub const fn with_max_retries(mut self, max_retries: usize) -> Self {
        self.max_retries = Some(max_retries);
        self
    }
}

#[async_trait]
impl<F, Fut> Tool for FunctionTool<F>
where
    F: Send + Sync + Fn(ToolContext, Value) -> Fut,
    Fut: Send + std::future::Future<Output = Result<ToolResult, ToolError>>,
{
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    fn parameters_schema(&self) -> Value {
        self.parameters.clone()
    }

    fn metadata(&self) -> Metadata {
        self.metadata.clone()
    }

    fn max_retries(&self) -> Option<usize> {
        self.max_retries
    }

    async fn call(&self, context: ToolContext, arguments: Value) -> Result<ToolResult, ToolError> {
        (self.function)(context, arguments).await
    }
}

/// Function-backed tool that derives its argument schema from a typed Rust input object.
pub struct TypedFunctionTool<Args, F> {
    name: String,
    description: Option<String>,
    parameters: Value,
    metadata: Metadata,
    max_retries: Option<usize>,
    function: F,
    _args: PhantomData<fn(Args)>,
}

fn normalize_tool_parameters_schema(parameters: &mut Value) {
    if !parameters.is_object() {
        *parameters = serde_json::json!({
            "type": "object",
            "properties": {},
        });
        return;
    }

    let Some(object) = parameters.as_object_mut() else {
        return;
    };
    object.remove("$schema");
    object
        .entry("type".to_string())
        .or_insert_with(|| Value::String("object".to_string()));
    object
        .entry("properties".to_string())
        .or_insert_with(|| serde_json::json!({}));
}

impl<Args, F> TypedFunctionTool<Args, F>
where
    Args: JsonSchema,
{
    /// Build a typed function-backed tool.
    ///
    /// Argument descriptions come from the `Args` [`JsonSchema`] implementation. With
    /// `#[derive(JsonSchema)]`, `schemars` maps Rust doc comments and `#[schemars(...)]` field
    /// attributes into each argument's JSON Schema without changing Serde deserialization.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        description: impl Into<Option<String>>,
        function: F,
    ) -> Self {
        let schema = schema_for!(Args);
        let mut parameters = serde_json::to_value(schema).unwrap_or_else(|_| {
            serde_json::json!({
                "type": "object",
                "properties": {},
            })
        });
        normalize_tool_parameters_schema(&mut parameters);
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
            metadata: Metadata::default(),
            max_retries: None,
            function,
            _args: PhantomData,
        }
    }

    /// Attach runtime metadata to this tool.
    #[must_use]
    pub fn with_metadata(mut self, metadata: Metadata) -> Self {
        self.metadata = metadata;
        self
    }

    /// Override the retry budget for this tool.
    #[must_use]
    pub const fn with_max_retries(mut self, max_retries: usize) -> Self {
        self.max_retries = Some(max_retries);
        self
    }
}

#[async_trait]
impl<Args, F, Fut> Tool for TypedFunctionTool<Args, F>
where
    Args: DeserializeOwned + JsonSchema + Send + 'static,
    F: Send + Sync + Fn(ToolContext, Args) -> Fut,
    Fut: Send + std::future::Future<Output = Result<ToolResult, ToolError>>,
{
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    fn parameters_schema(&self) -> Value {
        self.parameters.clone()
    }

    fn metadata(&self) -> Metadata {
        self.metadata.clone()
    }

    fn max_retries(&self) -> Option<usize> {
        self.max_retries
    }

    async fn call(&self, context: ToolContext, arguments: Value) -> Result<ToolResult, ToolError> {
        let args =
            serde_json::from_value(arguments).map_err(|error| ToolError::InvalidArguments {
                tool: self.name.clone(),
                message: error.to_string(),
            })?;
        (self.function)(context, args).await
    }
}

/// Create a plain JSON-returning tool from an async function.
#[must_use]
pub fn string_tool<F, Fut>(
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

/// Create a typed JSON-returning tool from an async function.
#[must_use]
pub fn typed_tool<Args, F, Fut>(
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
