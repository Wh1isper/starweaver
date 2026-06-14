use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{
    request::{OutputMode, PreparedInstruction},
    settings::ThinkingSettings,
    transport::HttpRequestOptions,
};

/// Request parameters derived from tools, output schemas, and runtime policy.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ModelRequestParameters {
    /// Tool definitions in provider-neutral JSON schema form.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolDefinition>,
    /// Provider-executed native tool definitions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub native_tools: Vec<NativeToolDefinition>,
    /// Optional output schema.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
    /// Selected output mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_mode: Option<OutputMode>,
    /// Prepared instruction fragments attached during request preparation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub instructions: Vec<PreparedInstruction>,
    /// Request-level thinking settings selected during request preparation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingSettings>,
    /// Allow text output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_text_output: Option<bool>,
    /// Allow image output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_image_output: Option<bool>,
    /// Request-level HTTP overrides for gateway, audit, and routing integrations.
    #[serde(default)]
    pub http: HttpRequestOptions,
    /// Provider-specific JSON object merged into the top-level request body.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub extra_body: Map<String, Value>,
    /// Request metadata for replay, trace, and audit.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub metadata: Map<String, Value>,
}

/// Provider-neutral tool definition.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolDefinition {
    /// Tool name.
    pub name: String,
    /// Tool description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON schema parameters.
    #[serde(default)]
    pub parameters: Value,
    /// Runtime metadata for capability hooks, filtering, approval, and provider adaptation.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub metadata: Map<String, Value>,
}

/// Provider-executed native tool definition.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NativeToolDefinition {
    /// Provider-neutral native tool type, such as `web_search` or `code_interpreter`.
    pub tool_type: String,
    /// Provider-specific native tool configuration.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub config: Map<String, Value>,
    /// Runtime metadata for capability hooks, filtering, and audit.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub metadata: Map<String, Value>,
}

impl NativeToolDefinition {
    /// Create a native tool definition.
    #[must_use]
    pub fn new(tool_type: impl Into<String>) -> Self {
        Self {
            tool_type: tool_type.into(),
            config: Map::new(),
            metadata: Map::new(),
        }
    }

    /// Attach provider-specific configuration.
    #[must_use]
    pub fn with_config(mut self, config: Map<String, Value>) -> Self {
        self.config = config;
        self
    }

    /// Attach runtime metadata.
    #[must_use]
    pub fn with_metadata(mut self, metadata: Map<String, Value>) -> Self {
        self.metadata = metadata;
        self
    }
}
