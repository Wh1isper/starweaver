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

/// MCP resource advertised by a server.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct McpResourceSpec {
    /// Resource URI or URI template.
    pub uri: String,
    /// Display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// MIME type when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    /// Resource metadata.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub metadata: Metadata,
}

impl McpResourceSpec {
    /// Create an MCP resource specification.
    #[must_use]
    pub fn new(uri: impl Into<String>) -> Self {
        Self {
            uri: uri.into(),
            name: None,
            description: None,
            mime_type: None,
            metadata: Metadata::default(),
        }
    }

    /// Attach a display name.
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Attach a description.
    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Attach a MIME type.
    #[must_use]
    pub fn with_mime_type(mut self, mime_type: impl Into<String>) -> Self {
        self.mime_type = Some(mime_type.into());
        self
    }

    /// Attach metadata.
    #[must_use]
    pub fn with_metadata(mut self, metadata: Metadata) -> Self {
        self.metadata = metadata;
        self
    }
}

/// MCP prompt advertised by a server.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct McpPromptSpec {
    /// Prompt name.
    pub name: String,
    /// Prompt description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON schema for prompt arguments.
    #[serde(default)]
    pub arguments: Value,
    /// Prompt metadata.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub metadata: Metadata,
}

impl McpPromptSpec {
    /// Create an MCP prompt specification.
    #[must_use]
    pub fn new(name: impl Into<String>, arguments: Value) -> Self {
        Self {
            name: name.into(),
            description: None,
            arguments,
            metadata: Metadata::default(),
        }
    }

    /// Attach a description.
    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Attach metadata.
    #[must_use]
    pub fn with_metadata(mut self, metadata: Metadata) -> Self {
        self.metadata = metadata;
        self
    }
}

/// MCP sampling capability advertised by a server.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct McpSamplingSpec {
    /// Whether model sampling callbacks are available.
    #[serde(default)]
    pub enabled: bool,
    /// Sampling metadata.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub metadata: Metadata,
}

impl McpSamplingSpec {
    /// Create a sampling capability specification.
    #[must_use]
    pub fn enabled() -> Self {
        Self {
            enabled: true,
            metadata: Metadata::default(),
        }
    }

    /// Attach metadata.
    #[must_use]
    pub fn with_metadata(mut self, metadata: Metadata) -> Self {
        self.metadata = metadata;
        self
    }
}

/// MCP subscription advertised by a server.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct McpSubscriptionSpec {
    /// Subscription name.
    pub name: String,
    /// Subscription target, such as a resource URI.
    pub target: String,
    /// Subscription metadata.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub metadata: Metadata,
}

impl McpSubscriptionSpec {
    /// Create an MCP subscription specification.
    #[must_use]
    pub fn new(name: impl Into<String>, target: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            target: target.into(),
            metadata: Metadata::default(),
        }
    }

    /// Attach metadata.
    #[must_use]
    pub fn with_metadata(mut self, metadata: Metadata) -> Self {
        self.metadata = metadata;
        self
    }
}

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
        return_schema: None,
        strict: None,
        sequential: None,
        metadata,
    }
}
