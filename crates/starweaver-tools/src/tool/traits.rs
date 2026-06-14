use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use starweaver_core::Metadata;
use starweaver_model::ToolDefinition;

use crate::{ToolContext, ToolError};

use super::ToolResult;

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
