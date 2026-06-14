//! Client-side MCP toolset and deferred tool calls.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use starweaver_core::Metadata;

use super::{McpToolSpec, McpToolsetConfig};
use crate::{DynTool, Tool, ToolContext, ToolError, ToolInstruction, ToolResult, Toolset};

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

    fn get_tools(&self) -> Vec<DynTool> {
        self.config
            .tools
            .iter()
            .cloned()
            .map(|spec| Arc::new(McpTool::new(self.config.clone(), spec)) as DynTool)
            .collect()
    }

    fn id(&self) -> Option<&str> {
        Some(&self.config.id)
    }

    fn get_instructions(&self) -> Vec<ToolInstruction> {
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
