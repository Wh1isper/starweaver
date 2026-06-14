//! MCP toolset foundations.

mod config;
mod native;
mod toolset;
mod transport;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use starweaver_core::Metadata;
use starweaver_model::ToolDefinition;

pub use config::McpToolsetConfig;
pub use native::NativeMcpServer;
pub use toolset::McpToolset;
pub use transport::McpTransport;

/// MCP client-side tool specification.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct McpToolSpec {
    /// Tool name declared by the MCP server.
    pub name: String,
    /// Tool description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON schema parameters.
    #[serde(default)]
    pub parameters: Value,
    /// Whether the MCP server declares task-augmented execution support for this tool.
    #[serde(default)]
    pub task: bool,
    /// Tool metadata.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub metadata: Metadata,
}

impl McpToolSpec {
    /// Create an MCP tool specification.
    #[must_use]
    pub fn new(name: impl Into<String>, parameters: Value) -> Self {
        Self {
            name: name.into(),
            description: None,
            parameters,
            task: false,
            metadata: Metadata::default(),
        }
    }

    /// Attach a description.
    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Mark task-augmented execution support.
    #[must_use]
    pub const fn with_task(mut self, task: bool) -> Self {
        self.task = task;
        self
    }

    /// Attach metadata.
    #[must_use]
    pub fn with_metadata(mut self, metadata: Metadata) -> Self {
        self.metadata = metadata;
        self
    }
}

/// Convert an MCP tool spec into a provider-neutral tool definition.
#[must_use]
pub fn tool_definition_from_mcp_spec(
    server_id: &str,
    transport: &McpTransport,
    spec: &McpToolSpec,
) -> ToolDefinition {
    let mut metadata = spec.metadata.clone();
    metadata.insert(
        "mcp_server_id".to_string(),
        Value::String(server_id.to_string()),
    );
    metadata.insert(
        "mcp_transport".to_string(),
        Value::String(transport.kind().to_string()),
    );
    metadata.insert(
        "mcp_tool_name".to_string(),
        Value::String(spec.name.clone()),
    );
    if spec.task {
        metadata.insert("mcp_task".to_string(), Value::Bool(true));
    }
    ToolDefinition {
        name: spec.name.clone(),
        description: spec.description.clone(),
        parameters: spec.parameters.clone(),
        metadata,
    }
}
