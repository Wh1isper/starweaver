//! Prefix wrappers for tools and toolsets.

use std::sync::Arc;

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

    fn timeout_ms(&self) -> Option<u64> {
        self.tool.timeout_ms()
    }

    fn return_schema(&self) -> Option<Value> {
        self.tool.return_schema()
    }

    fn strict_schema(&self) -> Option<bool> {
        self.tool.strict_schema()
    }

    fn sequential(&self) -> Option<bool> {
        self.tool.sequential()
    }

    fn is_available(&self, context: &AgentContext) -> bool {
        self.tool.is_available(context)
    }

    fn prepare_definition(
        &self,
        context: &AgentContext,
        definition: ToolDefinition,
    ) -> Option<ToolDefinition> {
        self.tool.prepare_definition(context, definition)
    }

    async fn call(&self, context: ToolContext, arguments: Value) -> Result<ToolResult, ToolError> {
        self.tool.call(context, arguments).await
    }

    async fn preprocess_user_input(
        &self,
        context: ToolContext,
        user_input: Value,
    ) -> Result<ToolUserInputPreprocessResult, ToolError> {
        self.tool.preprocess_user_input(context, user_input).await
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

    fn prefixed_tools(&self, tools: Vec<DynTool>) -> Vec<DynTool> {
        tools
            .into_iter()
            .map(|tool| Arc::new(PrefixedTool::new(self.prefix.clone(), tool)) as DynTool)
            .collect()
    }

    fn prefixed_instructions(&self, instructions: Vec<ToolInstruction>) -> Vec<ToolInstruction> {
        instructions
            .into_iter()
            .map(|instruction| ToolInstruction {
                group: prefixed_name(&self.prefix, &instruction.group),
                content: instruction.content,
                dynamic: instruction.dynamic,
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
        report.id = self.id().map(ToOwned::to_owned);
        report.tool_count = tool_count;
        report.instruction_count = instruction_count;
        report
    }
}

#[async_trait]
impl Toolset for PrefixedToolset {
    fn name(&self) -> &str {
        &self.name
    }

    fn get_tools(&self) -> Vec<DynTool> {
        self.prefixed_tools(self.toolset.get_tools())
    }

    fn max_retries(&self) -> Option<usize> {
        self.toolset.max_retries()
    }

    fn timeout_ms(&self) -> Option<u64> {
        self.toolset.timeout_ms()
    }

    fn id(&self) -> Option<&str> {
        Some(&self.name)
    }

    fn get_instructions(&self) -> Vec<ToolInstruction> {
        self.prefixed_instructions(self.toolset.get_instructions())
    }

    fn lifecycle_policy(&self) -> ToolsetLifecyclePolicy {
        self.toolset.lifecycle_policy()
    }

    async fn prepare_with_context(
        &self,
        context: &AgentContext,
    ) -> Result<ToolsetPreparation, ToolsetLifecycleError> {
        let preparation = self.toolset.prepare_with_context(context).await?;
        let tools = self.prefixed_tools(preparation.tools);
        let instructions = self.prefixed_instructions(preparation.instructions);
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
        let report = self.toolset.enter_with_context(context).await?;
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
        let report = self.toolset.exit_with_context(context).await?;
        Ok(self.wrapper_report(report, 0, 0))
    }
}

#[must_use]
pub(crate) fn prefixed_name(prefix: &str, name: &str) -> String {
    format!("{prefix}_{name}")
}
