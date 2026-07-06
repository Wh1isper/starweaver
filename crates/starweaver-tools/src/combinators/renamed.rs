use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use serde_json::Value;
use starweaver_context::AgentContext;
use starweaver_core::Metadata;
use starweaver_model::ToolDefinition;

use crate::{
    DynTool, DynToolset, Tool, ToolContext, ToolError, ToolInstruction, ToolResult,
    ToolUserInputPreprocessResult, Toolset, ToolsetLifecycleError, ToolsetLifecyclePolicy,
    ToolsetLifecycleReport, ToolsetPreparation,
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

    fn renamed_tools(&self, tools: Vec<DynTool>) -> Vec<DynTool> {
        tools
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

    fn wrapper_report(
        &self,
        mut report: ToolsetLifecycleReport,
        tool_count: usize,
        instruction_count: usize,
    ) -> ToolsetLifecycleReport {
        report.name.clone_from(&self.name);
        report.id.clone_from(&self.id);
        report.tool_count = tool_count;
        report.instruction_count = instruction_count;
        report
    }
}

#[async_trait]
impl Toolset for RenamedToolset {
    fn name(&self) -> &str {
        &self.name
    }

    fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    fn get_tools(&self) -> Vec<DynTool> {
        self.renamed_tools(self.inner.get_tools())
    }

    fn max_retries(&self) -> Option<usize> {
        self.inner.max_retries()
    }

    fn timeout_ms(&self) -> Option<u64> {
        self.inner.timeout_ms()
    }

    fn get_instructions(&self) -> Vec<ToolInstruction> {
        self.inner.get_instructions()
    }

    fn lifecycle_policy(&self) -> ToolsetLifecyclePolicy {
        self.inner.lifecycle_policy()
    }

    async fn prepare_with_context(
        &self,
        context: &AgentContext,
    ) -> Result<ToolsetPreparation, ToolsetLifecycleError> {
        let preparation = self.inner.prepare_with_context(context).await?;
        let tools = self.renamed_tools(preparation.tools);
        let instructions = preparation.instructions;
        let report = self.wrapper_report(preparation.report, tools.len(), instructions.len());
        Ok(ToolsetPreparation {
            tools,
            instructions,
            report,
        })
    }

    async fn enter_with_context(
        &self,
        context: &AgentContext,
    ) -> Result<ToolsetLifecycleReport, ToolsetLifecycleError> {
        let report = self.inner.enter_with_context(context).await?;
        Ok(self.wrapper_report(
            report,
            self.get_tools().len(),
            self.get_instructions().len(),
        ))
    }

    async fn exit_with_context(
        &self,
        context: &AgentContext,
    ) -> Result<ToolsetLifecycleReport, ToolsetLifecycleError> {
        let report = self.inner.exit_with_context(context).await?;
        Ok(self.wrapper_report(report, 0, 0))
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

    fn timeout_ms(&self) -> Option<u64> {
        self.inner.timeout_ms()
    }

    fn return_schema(&self) -> Option<Value> {
        self.inner.return_schema()
    }

    fn strict_schema(&self) -> Option<bool> {
        self.inner.strict_schema()
    }

    fn sequential(&self) -> Option<bool> {
        self.inner.sequential()
    }

    fn is_available(&self, context: &AgentContext) -> bool {
        self.inner.is_available(context)
    }

    fn prepare_definition(
        &self,
        context: &AgentContext,
        definition: ToolDefinition,
    ) -> Option<ToolDefinition> {
        self.inner.prepare_definition(context, definition)
    }

    async fn call(&self, context: ToolContext, arguments: Value) -> Result<ToolResult, ToolError> {
        self.inner.call(context, arguments).await
    }

    async fn preprocess_user_input(
        &self,
        context: ToolContext,
        user_input: Value,
    ) -> Result<ToolUserInputPreprocessResult, ToolError> {
        self.inner.preprocess_user_input(context, user_input).await
    }
}
