//! Toolset combinators for filtering, renaming, approval policy, preparation, and loading.

mod approval;
mod deferred;
mod renamed;

use std::{
    collections::BTreeSet,
    sync::{Arc, OnceLock},
};

use crate::{DynTool, DynToolset, Tool, ToolInstruction, Toolset};

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

    fn timeout_ms(&self) -> Option<u64> {
        self.inner.timeout_ms()
    }

    fn get_instructions(&self) -> Vec<ToolInstruction> {
        self.inner.get_instructions()
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
