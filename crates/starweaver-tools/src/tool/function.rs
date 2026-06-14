use async_trait::async_trait;
use serde_json::Value;
use starweaver_core::Metadata;

use crate::{ToolContext, ToolError};

use super::{Tool, ToolResult};

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
