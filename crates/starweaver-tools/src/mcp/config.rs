//! MCP toolset configuration.

use serde::{Deserialize, Serialize};

use super::{
    McpPromptSpec, McpResourceSpec, McpSamplingSpec, McpSubscriptionSpec, McpToolSpec, McpTransport,
};

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
    /// Declared server resources.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resources: Vec<McpResourceSpec>,
    /// Declared server prompts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prompts: Vec<McpPromptSpec>,
    /// Declared server sampling capability.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sampling: Option<McpSamplingSpec>,
    /// Declared server subscriptions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subscriptions: Vec<McpSubscriptionSpec>,
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
            resources: Vec::new(),
            prompts: Vec::new(),
            sampling: None,
            subscriptions: Vec::new(),
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

    /// Add one declared server resource.
    #[must_use]
    pub fn with_resource(mut self, resource: McpResourceSpec) -> Self {
        self.resources.push(resource);
        self
    }

    /// Add one declared server prompt.
    #[must_use]
    pub fn with_prompt(mut self, prompt: McpPromptSpec) -> Self {
        self.prompts.push(prompt);
        self
    }

    /// Attach server sampling capability.
    #[must_use]
    pub fn with_sampling(mut self, sampling: McpSamplingSpec) -> Self {
        self.sampling = Some(sampling);
        self
    }

    /// Add one declared server subscription.
    #[must_use]
    pub fn with_subscription(mut self, subscription: McpSubscriptionSpec) -> Self {
        self.subscriptions.push(subscription);
        self
    }
}

const fn default_cache_enabled() -> bool {
    true
}
