use std::{collections::BTreeSet, sync::Arc};

use crate::{DynTool, DynToolset, Tool, ToolInstruction, Toolset};

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
