//! MCP toolset foundations.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use starweaver_core::Metadata;
use starweaver_model::ToolDefinition;

use crate::{DynTool, Tool, ToolContext, ToolError, ToolInstruction, ToolResult, Toolset};

/// MCP client transport kind.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum McpTransport {
    /// Streamable HTTP transport.
    StreamableHttp {
        /// MCP endpoint URL.
        url: String,
        /// Optional HTTP headers.
        #[serde(default, skip_serializing_if = "Map::is_empty")]
        headers: Map<String, Value>,
    },
    /// Server-Sent Events transport.
    Sse {
        /// MCP endpoint URL.
        url: String,
        /// Optional HTTP headers.
        #[serde(default, skip_serializing_if = "Map::is_empty")]
        headers: Map<String, Value>,
    },
    /// Stdio subprocess transport.
    Stdio {
        /// Command to run.
        command: String,
        /// Command arguments.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        args: Vec<String>,
        /// Optional working directory.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        /// Optional subprocess environment.
        #[serde(default, skip_serializing_if = "Map::is_empty")]
        env: Map<String, Value>,
    },
}

impl McpTransport {
    /// Build a streamable HTTP transport.
    #[must_use]
    pub fn streamable_http(url: impl Into<String>) -> Self {
        Self::StreamableHttp {
            url: url.into(),
            headers: Map::new(),
        }
    }

    /// Build an SSE transport.
    #[must_use]
    pub fn sse(url: impl Into<String>) -> Self {
        Self::Sse {
            url: url.into(),
            headers: Map::new(),
        }
    }

    /// Build a stdio transport.
    #[must_use]
    pub fn stdio(command: impl Into<String>) -> Self {
        Self::Stdio {
            command: command.into(),
            args: Vec::new(),
            cwd: None,
            env: Map::new(),
        }
    }

    /// Attach transport headers to an HTTP transport.
    #[must_use]
    pub fn with_headers(mut self, headers: Map<String, Value>) -> Self {
        match &mut self {
            Self::StreamableHttp {
                headers: target, ..
            }
            | Self::Sse {
                headers: target, ..
            } => {
                *target = headers;
            }
            Self::Stdio { .. } => {}
        }
        self
    }

    /// Attach stdio command arguments.
    #[must_use]
    pub fn with_args(mut self, args: Vec<String>) -> Self {
        if let Self::Stdio { args: target, .. } = &mut self {
            *target = args;
        }
        self
    }

    /// Attach a stdio working directory.
    #[must_use]
    pub fn with_cwd(mut self, cwd: impl Into<String>) -> Self {
        if let Self::Stdio { cwd: target, .. } = &mut self {
            *target = Some(cwd.into());
        }
        self
    }

    /// Attach a stdio environment map.
    #[must_use]
    pub fn with_env(mut self, env: Map<String, Value>) -> Self {
        if let Self::Stdio { env: target, .. } = &mut self {
            *target = env;
        }
        self
    }

    /// Transport name used in metadata.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::StreamableHttp { .. } => "streamable_http",
            Self::Sse { .. } => "sse",
            Self::Stdio { .. } => "stdio",
        }
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
    #[serde(default, skip_serializing_if = "Map::is_empty")]
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

/// Client-side MCP toolset configuration.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct McpToolsetConfig {
    /// Toolset identifier.
    pub id: String,
    /// MCP client transport.
    pub transport: McpTransport,
    /// Include MCP server instructions in toolset instructions.
    #[serde(default)]
    pub include_instructions: bool,
    /// Cache listed tools between calls.
    #[serde(default = "default_cache_enabled")]
    pub cache_tools: bool,
    /// Tool name prefix used when callers want namespacing at the MCP layer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_prefix: Option<String>,
    /// Optional read timeout in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_timeout_ms: Option<u64>,
    /// Optional initialization timeout in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub init_timeout_ms: Option<u64>,
    /// Server instructions captured during initialization.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    /// Declared server tools.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<McpToolSpec>,
}

impl McpToolsetConfig {
    /// Create an MCP toolset configuration.
    #[must_use]
    pub fn new(id: impl Into<String>, transport: McpTransport) -> Self {
        Self {
            id: id.into(),
            transport,
            include_instructions: false,
            cache_tools: true,
            tool_prefix: None,
            read_timeout_ms: None,
            init_timeout_ms: None,
            instructions: None,
            tools: Vec::new(),
        }
    }

    /// Include server instructions.
    #[must_use]
    pub const fn with_include_instructions(mut self, include: bool) -> Self {
        self.include_instructions = include;
        self
    }

    /// Set tool caching behavior.
    #[must_use]
    pub const fn with_cache_tools(mut self, cache: bool) -> Self {
        self.cache_tools = cache;
        self
    }

    /// Set a tool prefix.
    #[must_use]
    pub fn with_tool_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.tool_prefix = Some(prefix.into());
        self
    }

    /// Set read timeout.
    #[must_use]
    pub const fn with_read_timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.read_timeout_ms = Some(timeout_ms);
        self
    }

    /// Set initialization timeout.
    #[must_use]
    pub const fn with_init_timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.init_timeout_ms = Some(timeout_ms);
        self
    }

    /// Attach server instructions.
    #[must_use]
    pub fn with_instructions(mut self, instructions: impl Into<String>) -> Self {
        self.instructions = Some(instructions.into());
        self
    }

    /// Add one declared server tool.
    #[must_use]
    pub fn with_tool(mut self, tool: McpToolSpec) -> Self {
        self.tools.push(tool);
        self
    }
}

/// Client-side MCP toolset foundation.
#[derive(Clone, Debug)]
pub struct McpToolset {
    config: McpToolsetConfig,
}

impl McpToolset {
    /// Create an MCP toolset.
    #[must_use]
    pub const fn new(config: McpToolsetConfig) -> Self {
        Self { config }
    }

    /// Return the toolset configuration.
    #[must_use]
    pub const fn config(&self) -> &McpToolsetConfig {
        &self.config
    }

    /// Return a stable conflict hint.
    #[must_use]
    pub const fn tool_name_conflict_hint(&self) -> &'static str {
        "wrap the MCP toolset in PrefixedToolset or set McpToolsetConfig::tool_prefix"
    }
}

impl Toolset for McpToolset {
    fn name(&self) -> &str {
        &self.config.id
    }

    fn tools(&self) -> Vec<DynTool> {
        self.config
            .tools
            .iter()
            .cloned()
            .map(|spec| Arc::new(McpTool::new(self.config.clone(), spec)) as DynTool)
            .collect()
    }

    fn instructions(&self) -> Vec<ToolInstruction> {
        if self.config.include_instructions {
            self.config
                .instructions
                .as_ref()
                .map(|instructions| {
                    ToolInstruction::new(format!("mcp:{}", self.config.id), instructions.clone())
                })
                .into_iter()
                .collect()
        } else {
            Vec::new()
        }
    }
}

#[derive(Clone, Debug)]
struct McpTool {
    config: McpToolsetConfig,
    spec: McpToolSpec,
    name: String,
}

impl McpTool {
    fn new(config: McpToolsetConfig, spec: McpToolSpec) -> Self {
        let name = config.tool_prefix.as_ref().map_or_else(
            || spec.name.clone(),
            |prefix| format!("{prefix}_{}", spec.name),
        );
        Self { config, spec, name }
    }
}

#[async_trait]
impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> Option<&str> {
        self.spec.description.as_deref()
    }

    fn parameters_schema(&self) -> Value {
        self.spec.parameters.clone()
    }

    fn metadata(&self) -> Metadata {
        let mut metadata = self.spec.metadata.clone();
        metadata.insert(
            "mcp_server_id".to_string(),
            Value::String(self.config.id.clone()),
        );
        metadata.insert(
            "mcp_transport".to_string(),
            Value::String(self.config.transport.kind().to_string()),
        );
        metadata.insert(
            "mcp_tool_name".to_string(),
            Value::String(self.spec.name.clone()),
        );
        if self.spec.task {
            metadata.insert("mcp_task".to_string(), Value::Bool(true));
        }
        metadata
    }

    async fn call(&self, _context: ToolContext, arguments: Value) -> Result<ToolResult, ToolError> {
        Err(ToolError::CallDeferred {
            tool: self.name.clone(),
            metadata: serde_json::json!({
                "kind": "mcp_tool_call",
                "server_id": self.config.id,
                "transport": self.config.transport,
                "tool_name": self.spec.name,
                "exposed_name": self.name,
                "arguments": arguments,
                "task": self.spec.task,
            }),
        })
    }
}

const fn default_cache_enabled() -> bool {
    true
}

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

/// Convert an MCP tool spec into a provider-neutral tool definition.
#[must_use]
pub fn mcp_tool_definition(
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
