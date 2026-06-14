use std::sync::Arc;

use crate::{DynTool, DynToolset, ToolInstruction, Toolset};

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
