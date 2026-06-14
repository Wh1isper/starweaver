use std::sync::Arc;

use crate::{DynTool, ToolInstruction, Toolset};

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
