use std::marker::PhantomData;

use async_trait::async_trait;
use schemars::{schema_for, JsonSchema};
use serde::de::DeserializeOwned;
use serde_json::Value;
use starweaver_core::Metadata;

use crate::{ToolContext, ToolError};

use super::{Tool, ToolResult};

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
