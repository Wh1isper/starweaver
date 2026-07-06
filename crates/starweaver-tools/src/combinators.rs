//! Toolset combinators for filtering, renaming, approval policy, preparation, and loading.

mod approval;
mod deferred;
mod renamed;

use std::{
    collections::BTreeSet,
    sync::{Arc, OnceLock},
};

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

pub use approval::ApprovalRequiredToolset;
pub use deferred::DeferredToolset;
pub use renamed::RenamedToolset;

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
    pub fn include_names(
        inner: DynToolset,
        names: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        let names = names.into_iter().map(Into::into).collect::<BTreeSet<_>>();
        Self::new(inner, Arc::new(move |tool| names.contains(tool.name())))
    }

    /// Build a toolset that removes listed tool names.
    #[must_use]
    pub fn exclude_names(
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

    fn filtered_tools(&self, tools: Vec<DynTool>) -> Vec<DynTool> {
        tools
            .into_iter()
            .filter(|tool| (self.predicate)(tool.as_ref()))
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
impl Toolset for FilteredToolset {
    fn name(&self) -> &str {
        &self.name
    }

    fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    fn get_tools(&self) -> Vec<DynTool> {
        self.filtered_tools(self.inner.get_tools())
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
        let tools = self.filtered_tools(preparation.tools);
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

/// Toolset wrapper that merges metadata into every exposed tool definition.
pub struct MetadataToolset {
    inner: DynToolset,
    name: String,
    id: Option<String>,
    metadata: Metadata,
}

impl MetadataToolset {
    /// Build a metadata wrapper.
    #[must_use]
    pub fn new(inner: DynToolset, metadata: Metadata) -> Self {
        let name = format!("{}_metadata", inner.name());
        let id = inner.id().map(|id| format!("{id}.metadata"));
        Self {
            inner,
            name,
            id,
            metadata,
        }
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

    fn metadata_tools(&self, tools: Vec<DynTool>) -> Vec<DynTool> {
        tools
            .into_iter()
            .map(|tool| {
                Arc::new(MetadataTool {
                    inner: tool,
                    metadata: self.metadata.clone(),
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
impl Toolset for MetadataToolset {
    fn name(&self) -> &str {
        &self.name
    }

    fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    fn get_tools(&self) -> Vec<DynTool> {
        self.metadata_tools(self.inner.get_tools())
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
        let tools = self.metadata_tools(preparation.tools);
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

struct MetadataTool {
    inner: DynTool,
    metadata: Metadata,
}

#[async_trait]
impl Tool for MetadataTool {
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
        metadata.extend(self.metadata.clone());
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

/// Toolset wrapper that combines several toolsets into one prepared inventory.
pub struct CombinedToolset {
    name: String,
    id: Option<String>,
    toolsets: Vec<DynToolset>,
    max_retries: Option<usize>,
    timeout_ms: Option<u64>,
}

impl CombinedToolset {
    /// Build a combined toolset.
    #[must_use]
    pub fn new(name: impl Into<String>, toolsets: Vec<DynToolset>) -> Self {
        Self {
            name: name.into(),
            id: None,
            toolsets,
            max_retries: None,
            timeout_ms: None,
        }
    }

    /// Set a stable identifier.
    #[must_use]
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Set retry default for member tools that do not override it.
    #[must_use]
    pub const fn with_max_retries(mut self, max_retries: usize) -> Self {
        self.max_retries = Some(max_retries);
        self
    }

    /// Set execution timeout default for member tools that do not override it.
    #[must_use]
    pub const fn with_timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = Some(timeout_ms);
        self
    }

    fn combined_tools(&self) -> Vec<DynTool> {
        self.toolsets
            .iter()
            .flat_map(|toolset| toolset.get_tools())
            .collect()
    }

    fn combined_instructions(&self) -> Vec<ToolInstruction> {
        self.toolsets
            .iter()
            .flat_map(|toolset| toolset.get_instructions())
            .collect()
    }
}

#[async_trait]
impl Toolset for CombinedToolset {
    fn name(&self) -> &str {
        &self.name
    }

    fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    fn get_tools(&self) -> Vec<DynTool> {
        self.combined_tools()
    }

    fn max_retries(&self) -> Option<usize> {
        self.max_retries
    }

    fn timeout_ms(&self) -> Option<u64> {
        self.timeout_ms
    }

    fn get_instructions(&self) -> Vec<ToolInstruction> {
        self.combined_instructions()
    }

    async fn prepare_with_context(
        &self,
        context: &AgentContext,
    ) -> Result<ToolsetPreparation, ToolsetLifecycleError> {
        let mut tools = Vec::new();
        let mut instructions = Vec::new();
        for toolset in &self.toolsets {
            let preparation = toolset.prepare_with_context(context).await?;
            tools.extend(preparation.tools);
            instructions.extend(preparation.instructions);
        }
        Ok(ToolsetPreparation::initialized(
            self.name.clone(),
            self.id.clone(),
            tools,
            instructions,
        ))
    }
}

/// Lazily loaded toolset that materializes an inner toolset on first use.
pub struct LazyToolset {
    name: String,
    id: Option<String>,
    load_toolset: Arc<dyn Fn() -> DynToolset + Send + Sync>,
    toolset: OnceLock<DynToolset>,
}

impl LazyToolset {
    /// Build a lazily loaded toolset.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        load_toolset: impl Fn() -> DynToolset + Send + Sync + 'static,
    ) -> Self {
        Self {
            name: name.into(),
            id: None,
            load_toolset: Arc::new(load_toolset),
            toolset: OnceLock::new(),
        }
    }

    /// Set a stable identifier.
    #[must_use]
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    fn loaded_toolset(&self) -> &DynToolset {
        self.toolset.get_or_init(|| (self.load_toolset)())
    }
}

impl Toolset for LazyToolset {
    fn name(&self) -> &str {
        &self.name
    }

    fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    fn get_tools(&self) -> Vec<DynTool> {
        self.loaded_toolset().get_tools()
    }

    fn max_retries(&self) -> Option<usize> {
        self.loaded_toolset().max_retries()
    }

    fn timeout_ms(&self) -> Option<u64> {
        self.loaded_toolset().timeout_ms()
    }

    fn get_instructions(&self) -> Vec<ToolInstruction> {
        self.loaded_toolset().get_instructions()
    }
}

/// Toolset backed by callbacks so hosts can expose dynamic tool inventories.
pub struct DynamicToolset {
    name: String,
    id: Option<String>,
    load_tools: Arc<dyn Fn() -> Vec<DynTool> + Send + Sync>,
    load_instructions: Arc<dyn Fn() -> Vec<ToolInstruction> + Send + Sync>,
    max_retries: Option<usize>,
}

impl DynamicToolset {
    /// Build a dynamic toolset.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        load_tools: impl Fn() -> Vec<DynTool> + Send + Sync + 'static,
    ) -> Self {
        Self {
            name: name.into(),
            id: None,
            load_tools: Arc::new(load_tools),
            load_instructions: Arc::new(Vec::new),
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
        load_instructions: impl Fn() -> Vec<ToolInstruction> + Send + Sync + 'static,
    ) -> Self {
        self.load_instructions = Arc::new(load_instructions);
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
        (self.load_tools)()
    }

    fn max_retries(&self) -> Option<usize> {
        self.max_retries
    }

    fn timeout_ms(&self) -> Option<u64> {
        None
    }

    fn get_instructions(&self) -> Vec<ToolInstruction> {
        (self.load_instructions)()
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
