use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use serde_json::Value;
use starweaver_core::Metadata;

use crate::{
    DynTool, DynToolset, Tool, ToolContext, ToolError, ToolInstruction, ToolResult, Toolset,
};

/// Toolset wrapper that applies stable tool name mappings.
pub struct RenamedToolset {
    inner: DynToolset,
    name: String,
    id: Option<String>,
    mappings: BTreeMap<String, String>,
}

impl RenamedToolset {
    /// Build a renamed toolset from original-name to exposed-name mappings.
    #[must_use]
    pub fn new(inner: DynToolset, mappings: impl IntoIterator<Item = (String, String)>) -> Self {
        let name = format!("{}_renamed", inner.name());
        let id = inner.id().map(|id| format!("{id}.renamed"));
        Self {
            inner,
            name,
            id,
            mappings: mappings.into_iter().collect(),
        }
    }

    /// Build a renamed toolset from borrowed string pairs.
    #[must_use]
    pub fn from_pairs(
        inner: DynToolset,
        mappings: impl IntoIterator<Item = (&'static str, &'static str)>,
    ) -> Self {
        Self::new(
            inner,
            mappings
                .into_iter()
                .map(|(from, to)| (from.to_string(), to.to_string())),
        )
    }

    /// Override wrapper name.
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Override wrapper id.
    #[must_use]
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }
}

impl Toolset for RenamedToolset {
    fn name(&self) -> &str {
        &self.name
    }

    fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    fn get_tools(&self) -> Vec<DynTool> {
        self.inner
            .get_tools()
            .into_iter()
            .map(|tool| {
                if let Some(name) = self.mappings.get(tool.name()) {
                    Arc::new(RenamedTool {
                        inner: tool,
                        name: name.clone(),
                    }) as DynTool
                } else {
                    tool
                }
            })
            .collect()
    }

    fn max_retries(&self) -> Option<usize> {
        self.inner.max_retries()
    }

    fn get_instructions(&self) -> Vec<ToolInstruction> {
        self.inner.get_instructions()
    }
}

struct RenamedTool {
    inner: DynTool,
    name: String,
}

#[async_trait]
impl Tool for RenamedTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> Option<&str> {
        self.inner.description()
    }

    fn parameters_schema(&self) -> Value {
        self.inner.parameters_schema()
    }

    fn metadata(&self) -> Metadata {
        let mut metadata = self.inner.metadata();
        metadata.insert(
            "original_tool_name".to_string(),
            Value::String(self.inner.name().to_string()),
        );
        metadata
    }

    fn max_retries(&self) -> Option<usize> {
        self.inner.max_retries()
    }

    async fn call(&self, context: ToolContext, arguments: Value) -> Result<ToolResult, ToolError> {
        self.inner.call(context, arguments).await
    }
}
