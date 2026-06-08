//! Toolset combinators for filtering, renaming, approval policy, preparation, and loading.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, OnceLock},
};

use async_trait::async_trait;
use serde_json::Value;
use starweaver_core::Metadata;

use crate::{
    DynTool, DynToolset, Tool, ToolApprovalState, ToolContext, ToolError, ToolInstruction,
    ToolResult, Toolset,
};

/// Predicate used by [`FilteredToolset`].
pub type ToolPredicate = Arc<dyn Fn(&dyn Tool) -> bool + Send + Sync>;

/// Toolset wrapper that exposes only tools accepted by a predicate.
pub struct FilteredToolset {
    inner: DynToolset,
    name: String,
    id: Option<String>,
    predicate: ToolPredicate,
}

impl FilteredToolset {
    /// Build a filtered toolset.
    #[must_use]
    pub fn new(inner: DynToolset, predicate: ToolPredicate) -> Self {
        let name = format!("{}_filtered", inner.name());
        let id = inner.id().map(|id| format!("{id}.filtered"));
        Self {
            inner,
            name,
            id,
            predicate,
        }
    }

    /// Build a toolset that keeps only listed tool names.
    #[must_use]
    pub fn allow_names(
        inner: DynToolset,
        names: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        let names = names.into_iter().map(Into::into).collect::<BTreeSet<_>>();
        Self::new(inner, Arc::new(move |tool| names.contains(tool.name())))
    }

    /// Build a toolset that removes listed tool names.
    #[must_use]
    pub fn deny_names(
        inner: DynToolset,
        names: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        let names = names.into_iter().map(Into::into).collect::<BTreeSet<_>>();
        Self::new(inner, Arc::new(move |tool| !names.contains(tool.name())))
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

impl Toolset for FilteredToolset {
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
            .filter(|tool| (self.predicate)(tool.as_ref()))
            .collect()
    }

    fn max_retries(&self) -> Option<usize> {
        self.inner.max_retries()
    }

    fn get_instructions(&self) -> Vec<ToolInstruction> {
        self.inner.get_instructions()
    }
}

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

/// Context-free preparation wrapper for tools and instructions.
pub struct PreparedToolset {
    inner: DynToolset,
    name: String,
    id: Option<String>,
    prepare_tools: Arc<dyn Fn(Vec<DynTool>) -> Vec<DynTool> + Send + Sync>,
    prepare_instructions: Arc<dyn Fn(Vec<ToolInstruction>) -> Vec<ToolInstruction> + Send + Sync>,
}

impl PreparedToolset {
    /// Build a prepared toolset.
    #[must_use]
    pub fn new(
        inner: DynToolset,
        prepare_tools: impl Fn(Vec<DynTool>) -> Vec<DynTool> + Send + Sync + 'static,
    ) -> Self {
        let name = format!("{}_prepared", inner.name());
        let id = inner.id().map(|id| format!("{id}.prepared"));
        Self {
            inner,
            name,
            id,
            prepare_tools: Arc::new(prepare_tools),
            prepare_instructions: Arc::new(|instructions| instructions),
        }
    }

    /// Attach an instruction preparation callback.
    #[must_use]
    pub fn with_instruction_prepare(
        mut self,
        prepare_instructions: impl Fn(Vec<ToolInstruction>) -> Vec<ToolInstruction>
            + Send
            + Sync
            + 'static,
    ) -> Self {
        self.prepare_instructions = Arc::new(prepare_instructions);
        self
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

impl Toolset for PreparedToolset {
    fn name(&self) -> &str {
        &self.name
    }

    fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    fn get_tools(&self) -> Vec<DynTool> {
        (self.prepare_tools)(self.inner.get_tools())
    }

    fn max_retries(&self) -> Option<usize> {
        self.inner.max_retries()
    }

    fn get_instructions(&self) -> Vec<ToolInstruction> {
        (self.prepare_instructions)(self.inner.get_instructions())
    }
}

/// Toolset backed by callbacks so hosts can expose dynamic tool inventories.
pub struct DynamicToolset {
    name: String,
    id: Option<String>,
    get_tools: Arc<dyn Fn() -> Vec<DynTool> + Send + Sync>,
    get_instructions: Arc<dyn Fn() -> Vec<ToolInstruction> + Send + Sync>,
    max_retries: Option<usize>,
}

impl DynamicToolset {
    /// Build a dynamic toolset.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        get_tools: impl Fn() -> Vec<DynTool> + Send + Sync + 'static,
    ) -> Self {
        Self {
            name: name.into(),
            id: None,
            get_tools: Arc::new(get_tools),
            get_instructions: Arc::new(Vec::new),
            max_retries: None,
        }
    }

    /// Set a stable identifier.
    #[must_use]
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Set dynamic instructions callback.
    #[must_use]
    pub fn with_instructions(
        mut self,
        get_instructions: impl Fn() -> Vec<ToolInstruction> + Send + Sync + 'static,
    ) -> Self {
        self.get_instructions = Arc::new(get_instructions);
        self
    }

    /// Set retry default.
    #[must_use]
    pub const fn with_max_retries(mut self, max_retries: usize) -> Self {
        self.max_retries = Some(max_retries);
        self
    }
}

impl Toolset for DynamicToolset {
    fn name(&self) -> &str {
        &self.name
    }

    fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    fn get_tools(&self) -> Vec<DynTool> {
        (self.get_tools)()
    }

    fn max_retries(&self) -> Option<usize> {
        self.max_retries
    }

    fn get_instructions(&self) -> Vec<ToolInstruction> {
        (self.get_instructions)()
    }
}

/// Lazily loaded toolset that materializes an inner toolset on first use.
pub struct DeferredLoadingToolset {
    name: String,
    id: Option<String>,
    loader: Arc<dyn Fn() -> DynToolset + Send + Sync>,
    inner: OnceLock<DynToolset>,
}

impl DeferredLoadingToolset {
    /// Build a deferred-loading toolset.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        loader: impl Fn() -> DynToolset + Send + Sync + 'static,
    ) -> Self {
        Self {
            name: name.into(),
            id: None,
            loader: Arc::new(loader),
            inner: OnceLock::new(),
        }
    }

    /// Set a stable identifier.
    #[must_use]
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    fn inner(&self) -> &DynToolset {
        self.inner.get_or_init(|| (self.loader)())
    }
}

impl Toolset for DeferredLoadingToolset {
    fn name(&self) -> &str {
        &self.name
    }

    fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    fn get_tools(&self) -> Vec<DynTool> {
        self.inner().get_tools()
    }

    fn max_retries(&self) -> Option<usize> {
        self.inner().max_retries()
    }

    fn get_instructions(&self) -> Vec<ToolInstruction> {
        self.inner().get_instructions()
    }
}
