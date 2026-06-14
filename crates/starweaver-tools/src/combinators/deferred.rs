use std::sync::{Arc, OnceLock};

use crate::{DynTool, DynToolset, ToolInstruction, Toolset};

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
