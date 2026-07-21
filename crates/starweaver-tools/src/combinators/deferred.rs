use std::{collections::BTreeSet, sync::Arc};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use starweaver_context::AgentContext;
use starweaver_core::Metadata;
use starweaver_model::ToolDefinition;

use crate::{
    DynTool, DynToolset, StaticToolset, Tool, ToolContext, ToolError, ToolInstruction, ToolKind,
    ToolResult, ToolUserInputPreprocessResult, Toolset, ToolsetLifecycleError,
    ToolsetLifecyclePolicy, ToolsetLifecycleReport, ToolsetPreparation, set_tool_metadata_kind,
};

/// Declarative model-facing tool that is always executed by an external client.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DeferredToolSpec {
    /// Unique model-facing tool name.
    pub name: String,
    /// Model-facing tool description.
    pub description: String,
    /// JSON schema for tool arguments.
    pub parameters: Value,
    /// Additional model instructions associated with this tool.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub instructions: Vec<String>,
}

impl DeferredToolSpec {
    /// Build a declarative deferred tool specification.
    #[must_use]
    pub fn new(name: impl Into<String>, description: impl Into<String>, parameters: Value) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
            instructions: Vec::new(),
        }
    }

    /// Add one model instruction for this tool.
    #[must_use]
    pub fn with_instruction(mut self, instruction: impl Into<String>) -> Self {
        self.instructions.push(instruction.into());
        self
    }
}

/// Toolset wrapper that marks matching tools as deferred external work.
pub struct DeferredToolset {
    inner: DynToolset,
    name: String,
    id: Option<String>,
    deferred: BTreeSet<String>,
    reason: String,
}

impl DeferredToolset {
    /// Build a deferred-call wrapper. Entries can match tool name, toolset name/id, metadata
    /// `bundle`, or `*`.
    #[must_use]
    pub fn new(inner: DynToolset, deferred: impl IntoIterator<Item = impl Into<String>>) -> Self {
        let name = format!("{}_deferred", inner.name());
        let id = inner.id().map(|id| format!("{id}.deferred"));
        Self {
            inner,
            name,
            id,
            deferred: deferred.into_iter().map(Into::into).collect(),
            reason: "configured deferred tool policy".to_string(),
        }
    }

    /// Build a toolset directly from client-supplied declarative tool definitions.
    ///
    /// Calls are never executed in-process. The first call suspends the run, while a resumed call
    /// returns the matching externally supplied deferred result.
    #[must_use]
    pub fn from_specs(
        name: impl Into<String>,
        specs: impl IntoIterator<Item = DeferredToolSpec>,
    ) -> Self {
        let name = name.into();
        let mut inner = StaticToolset::new(&name).with_id(&name);
        for spec in specs {
            for (index, instruction) in spec.instructions.iter().enumerate() {
                inner = inner.with_instruction(
                    ToolInstruction::new(
                        format!("{name}.{}.{}", spec.name, index + 1),
                        instruction,
                    )
                    .with_dynamic(true),
                );
            }
            inner = inner.with_tool(Arc::new(DeclaredDeferredTool { spec }));
        }
        Self::all(Arc::new(inner))
            .with_name(name.clone())
            .with_id(name)
            .with_reason("client-managed deferred tool")
    }

    /// Defer all tools in the inner toolset.
    #[must_use]
    pub fn all(inner: DynToolset) -> Self {
        Self::new(inner, ["*"])
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

    /// Override deferred-call reason.
    #[must_use]
    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = reason.into();
        self
    }

    fn wrapped_tools(&self, tools: Vec<DynTool>) -> Vec<DynTool> {
        let toolset_key = self
            .inner
            .id()
            .unwrap_or_else(|| self.inner.name())
            .to_string();
        tools
            .into_iter()
            .map(|tool| {
                Arc::new(DeferredTool {
                    inner: tool,
                    toolset_key: toolset_key.clone(),
                    deferred: self.deferred.clone(),
                    reason: self.reason.clone(),
                }) as DynTool
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
impl Toolset for DeferredToolset {
    fn name(&self) -> &str {
        &self.name
    }

    fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    fn get_tools(&self) -> Vec<DynTool> {
        self.wrapped_tools(self.inner.get_tools())
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
        let tools = self.wrapped_tools(preparation.tools);
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

struct DeclaredDeferredTool {
    spec: DeferredToolSpec,
}

#[async_trait]
impl Tool for DeclaredDeferredTool {
    fn name(&self) -> &str {
        &self.spec.name
    }

    fn description(&self) -> Option<&str> {
        Some(&self.spec.description)
    }

    fn parameters_schema(&self) -> Value {
        self.spec.parameters.clone()
    }

    fn metadata(&self) -> Metadata {
        let mut metadata = Metadata::default();
        metadata.insert("deferred_call".to_string(), Value::Bool(true));
        set_tool_metadata_kind(&mut metadata, ToolKind::Deferred);
        metadata
    }

    async fn call(
        &self,
        _context: ToolContext,
        _arguments: Value,
    ) -> Result<ToolResult, ToolError> {
        Err(ToolError::Execution {
            tool: self.spec.name.clone(),
            message: "declared deferred tool must be wrapped by DeferredToolset".to_string(),
        })
    }
}

struct DeferredTool {
    inner: DynTool,
    toolset_key: String,
    deferred: BTreeSet<String>,
    reason: String,
}

impl DeferredTool {
    fn is_deferred(&self) -> bool {
        let metadata = self.inner.metadata();
        self.deferred.contains("*")
            || self.deferred.contains(self.inner.name())
            || self.deferred.contains(&self.toolset_key)
            || metadata
                .get("bundle")
                .and_then(Value::as_str)
                .is_some_and(|bundle| self.deferred.contains(bundle))
    }
}

#[async_trait]
impl Tool for DeferredTool {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn description(&self) -> Option<&str> {
        self.inner.description()
    }

    fn parameters_schema(&self) -> Value {
        self.inner.parameters_schema()
    }

    fn metadata(&self) -> Metadata {
        let mut metadata = self.inner.metadata();
        if self.is_deferred() {
            metadata.insert("deferred_call".to_string(), Value::Bool(true));
            set_tool_metadata_kind(&mut metadata, ToolKind::Deferred);
        }
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
        let mut definition = self.inner.prepare_definition(context, definition)?;
        if self.is_deferred() {
            definition
                .metadata
                .insert("deferred_call".to_string(), Value::Bool(true));
            definition.metadata.insert(
                crate::TOOL_METADATA_KIND_KEY.to_string(),
                Value::String(ToolKind::Deferred.as_str().to_string()),
            );
        }
        Some(definition)
    }

    async fn call(&self, context: ToolContext, arguments: Value) -> Result<ToolResult, ToolError> {
        if !self.is_deferred() {
            return self.inner.call(context, arguments).await;
        }
        if let Some(result) = context.deferred_result {
            let mut tool_result = ToolResult::new(result);
            tool_result.metadata.insert(
                "deferred_state".to_string(),
                Value::String("completed".to_string()),
            );
            return Ok(tool_result);
        }
        let tool_call_id = context.metadata.get("tool_call_id").and_then(Value::as_str);
        let deferred_id = tool_call_id
            .map(|tool_call_id| format!("deferred_{}_{}", context.run_id.as_str(), tool_call_id));
        Err(ToolError::CallDeferred {
            tool: self.name().to_string(),
            metadata: serde_json::json!({
                "kind": "client_tool_call",
                "deferred_id": deferred_id,
                "tool_call_id": tool_call_id,
                "tool_name": self.name(),
                "arguments": arguments,
                "reason": self.reason,
                "toolset": self.toolset_key,
            }),
        })
    }

    async fn preprocess_user_input(
        &self,
        context: ToolContext,
        user_input: Value,
    ) -> Result<ToolUserInputPreprocessResult, ToolError> {
        self.inner.preprocess_user_input(context, user_input).await
    }
}
