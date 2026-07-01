use std::{collections::BTreeSet, sync::Arc};

use async_trait::async_trait;
use serde_json::Value;
use starweaver_context::AgentContext;
use starweaver_core::Metadata;
use starweaver_model::ToolDefinition;

use crate::{
    DynTool, DynToolset, Tool, ToolContext, ToolError, ToolInstruction, ToolKind, ToolResult,
    ToolUserInputPreprocessResult, Toolset, set_tool_metadata_kind,
};

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
}

impl Toolset for DeferredToolset {
    fn name(&self) -> &str {
        &self.name
    }

    fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    fn get_tools(&self) -> Vec<DynTool> {
        let toolset_key = self
            .inner
            .id()
            .unwrap_or_else(|| self.inner.name())
            .to_string();
        self.inner
            .get_tools()
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

    fn max_retries(&self) -> Option<usize> {
        self.inner.max_retries()
    }

    fn get_instructions(&self) -> Vec<ToolInstruction> {
        self.inner.get_instructions()
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
        Err(ToolError::CallDeferred {
            tool: self.name().to_string(),
            metadata: serde_json::json!({
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
