//! Provider-native remote MCP server definitions.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use starweaver_core::Metadata;

/// Provider-native remote MCP server definition.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NativeMcpServer {
    /// Provider-facing server label.
    pub id: String,
    /// Public MCP server URL or provider connector URI.
    pub url: String,
    /// Optional authorization token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization_token: Option<String>,
    /// Optional server description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Optional allow-list for server tools.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<Vec<String>>,
    /// Optional headers for providers that support remote MCP headers.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub headers: Map<String, Value>,
    /// Runtime metadata for hooks and audit.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub metadata: Metadata,
}

impl NativeMcpServer {
    /// Create a provider-native MCP server definition.
    #[must_use]
    pub fn new(id: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            url: url.into(),
            authorization_token: None,
            description: None,
            allowed_tools: None,
            headers: Map::new(),
            metadata: Metadata::default(),
        }
    }

    /// Attach an authorization token.
    #[must_use]
    pub fn with_authorization_token(mut self, token: impl Into<String>) -> Self {
        self.authorization_token = Some(token.into());
        self
    }

    /// Attach a server description.
    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Attach allowed tool names.
    #[must_use]
    pub fn with_allowed_tools(mut self, tools: Vec<String>) -> Self {
        self.allowed_tools = Some(tools);
        self
    }

    /// Attach HTTP headers.
    #[must_use]
    pub fn with_headers(mut self, headers: Map<String, Value>) -> Self {
        self.headers = headers;
        self
    }

    /// Attach runtime metadata.
    #[must_use]
    pub fn with_metadata(mut self, metadata: Metadata) -> Self {
        self.metadata = metadata;
        self
    }

    /// Convert into a provider-native tool definition for model request parameters.
    #[must_use]
    pub fn native_tool_definition(&self) -> starweaver_model::NativeToolDefinition {
        let mut config = Map::new();
        config.insert("server_label".to_string(), Value::String(self.id.clone()));
        if self.url.starts_with("x-openai-connector:") {
            if let Some((_, connector_id)) = self.url.split_once(':') {
                config.insert(
                    "connector_id".to_string(),
                    Value::String(connector_id.to_string()),
                );
            }
        } else {
            config.insert("server_url".to_string(), Value::String(self.url.clone()));
        }
        config.insert(
            "require_approval".to_string(),
            Value::String("never".to_string()),
        );
        if let Some(token) = &self.authorization_token {
            config.insert("authorization".to_string(), Value::String(token.clone()));
        }
        if let Some(description) = &self.description {
            config.insert(
                "server_description".to_string(),
                Value::String(description.clone()),
            );
        }
        if let Some(tools) = &self.allowed_tools {
            config.insert("allowed_tools".to_string(), serde_json::json!(tools));
        }
        if !self.headers.is_empty() {
            config.insert("headers".to_string(), Value::Object(self.headers.clone()));
        }
        starweaver_model::NativeToolDefinition::new("mcp")
            .with_config(config)
            .with_metadata(self.metadata.clone())
    }
}
