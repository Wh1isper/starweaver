//! Prefix wrappers for tools and toolsets.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use starweaver_core::Metadata;

use crate::{
    DynTool, DynToolset, Tool, ToolContext, ToolError, ToolInstruction, ToolResult, Toolset,
};

/// Tool wrapper that prefixes the exposed tool name and delegates execution to the wrapped tool.
#[derive(Clone)]
pub struct PrefixedTool {
    prefix: String,
    tool: DynTool,
    name: String,
}

impl PrefixedTool {
    /// Wrap a tool with a name prefix.
    #[must_use]
    pub fn new(prefix: impl Into<String>, tool: DynTool) -> Self {
        let prefix = prefix.into();
        let name = prefixed_name(&prefix, tool.name());
        Self { prefix, tool, name }
    }

    /// Prefix used by this wrapper.
    #[must_use]
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// Wrapped tool.
    #[must_use]
    pub fn inner(&self) -> DynTool {
        self.tool.clone()
    }
}

#[async_trait]
impl Tool for PrefixedTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> Option<&str> {
        self.tool.description()
    }

    fn parameters_schema(&self) -> Value {
        self.tool.parameters_schema()
    }

    fn metadata(&self) -> Metadata {
        self.tool.metadata()
    }

    fn max_retries(&self) -> Option<usize> {
        self.tool.max_retries()
    }

    async fn call(&self, context: ToolContext, arguments: Value) -> Result<ToolResult, ToolError> {
        self.tool.call(context, arguments).await
    }
}

/// Toolset wrapper that prefixes exposed tool names and delegates execution to wrapped tools.
#[derive(Clone)]
pub struct PrefixedToolset {
    prefix: String,
    toolset: DynToolset,
    name: String,
}

impl PrefixedToolset {
    /// Wrap a toolset with a name prefix.
    #[must_use]
    pub fn new(prefix: impl Into<String>, toolset: DynToolset) -> Self {
        let prefix = prefix.into();
        let name = prefixed_name(&prefix, toolset.name());
        Self {
            prefix,
            toolset,
            name,
        }
    }

    /// Prefix used by this wrapper.
    #[must_use]
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// Wrapped toolset.
    #[must_use]
    pub fn inner(&self) -> DynToolset {
        self.toolset.clone()
    }
}

impl Toolset for PrefixedToolset {
    fn name(&self) -> &str {
        &self.name
    }

    fn tools(&self) -> Vec<DynTool> {
        self.toolset
            .tools()
            .into_iter()
            .map(|tool| Arc::new(PrefixedTool::new(self.prefix.clone(), tool)) as DynTool)
            .collect()
    }

    fn max_retries(&self) -> Option<usize> {
        self.toolset.max_retries()
    }

    fn instructions(&self) -> Vec<ToolInstruction> {
        self.toolset
            .instructions()
            .into_iter()
            .map(|instruction| ToolInstruction {
                group: prefixed_name(&self.prefix, &instruction.group),
                content: instruction.content,
            })
            .collect()
    }
}

#[must_use]
pub(crate) fn prefixed_name(prefix: &str, name: &str) -> String {
    format!("{prefix}_{name}")
}
