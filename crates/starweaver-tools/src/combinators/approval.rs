use std::{collections::BTreeSet, sync::Arc};

use async_trait::async_trait;
use serde_json::Value;
use starweaver_core::Metadata;

use crate::{
    DynTool, DynToolset, Tool, ToolApprovalState, ToolContext, ToolError, ToolInstruction,
    ToolResult, Toolset,
};

/// Toolset wrapper that marks and gates tools through approval control flow.
pub struct ApprovalRequiredToolset {
    inner: DynToolset,
    name: String,
    id: Option<String>,
    approval: BTreeSet<String>,
    reason: String,
}

impl ApprovalRequiredToolset {
    /// Build an approval wrapper. Entries can match tool name, toolset name/id, metadata `bundle`, or `*`.
    #[must_use]
    pub fn new(inner: DynToolset, approval: impl IntoIterator<Item = impl Into<String>>) -> Self {
        let name = format!("{}_approval_required", inner.name());
        let id = inner.id().map(|id| format!("{id}.approval_required"));
        Self {
            inner,
            name,
            id,
            approval: approval.into_iter().map(Into::into).collect(),
            reason: "configured tool approval policy".to_string(),
        }
    }

    /// Require approval for all tools in the inner toolset.
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

    /// Override approval reason.
    #[must_use]
    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = reason.into();
        self
    }
}

impl Toolset for ApprovalRequiredToolset {
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
                Arc::new(ApprovalRequiredTool {
                    inner: tool,
                    toolset_key: toolset_key.clone(),
                    approval: self.approval.clone(),
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

struct ApprovalRequiredTool {
    inner: DynTool,
    toolset_key: String,
    approval: BTreeSet<String>,
    reason: String,
}

impl ApprovalRequiredTool {
    fn requires_approval(&self) -> bool {
        let metadata = self.inner.metadata();
        self.approval.contains("*")
            || self.approval.contains(self.inner.name())
            || self.approval.contains(&self.toolset_key)
            || metadata
                .get("bundle")
                .and_then(Value::as_str)
                .is_some_and(|bundle| self.approval.contains(bundle))
    }
}

#[async_trait]
impl Tool for ApprovalRequiredTool {
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
        if self.requires_approval() {
            metadata.insert("approval_required".to_string(), Value::Bool(true));
        }
        metadata
    }

    fn max_retries(&self) -> Option<usize> {
        self.inner.max_retries()
    }

    async fn call(&self, context: ToolContext, arguments: Value) -> Result<ToolResult, ToolError> {
        if self.requires_approval() {
            let approval = context.approval.clone();
            match approval {
                Some(ToolApprovalState::Approved {
                    override_arguments,
                    metadata,
                }) => {
                    let execution_arguments =
                        override_arguments.unwrap_or_else(|| arguments.clone());
                    let mut result = self.inner.call(context, execution_arguments).await?;
                    result.metadata.insert(
                        "approval_state".to_string(),
                        Value::String("approved".to_string()),
                    );
                    if !metadata.is_empty() {
                        result.metadata.insert(
                            "approval_metadata".to_string(),
                            Value::Object(metadata.clone()),
                        );
                    }
                    return Ok(result);
                }
                Some(ToolApprovalState::Denied { reason, metadata }) => {
                    let mut denial = serde_json::Map::new();
                    denial.insert("arguments".to_string(), arguments);
                    denial.insert("reason".to_string(), serde_json::json!(reason));
                    denial.insert(
                        "toolset".to_string(),
                        Value::String(self.toolset_key.clone()),
                    );
                    if !metadata.is_empty() {
                        denial.insert("metadata".to_string(), Value::Object(metadata));
                    }
                    return Err(ToolError::ApprovalRequired {
                        tool: self.name().to_string(),
                        metadata: Value::Object(denial),
                    });
                }
                None => {
                    return Err(ToolError::ApprovalRequired {
                        tool: self.name().to_string(),
                        metadata: serde_json::json!({
                            "arguments": arguments,
                            "reason": self.reason,
                            "toolset": self.toolset_key,
                        }),
                    });
                }
            }
        }
        self.inner.call(context, arguments).await
    }
}
