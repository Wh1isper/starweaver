//! Tool execution context.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use starweaver_context::DependencyStore;
use starweaver_core::{ConversationId, Metadata, RunId};

/// Context passed into tool execution.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ToolContext {
    /// Current run identifier.
    pub run_id: RunId,
    /// Current conversation identifier.
    pub conversation_id: ConversationId,
    /// Current run step.
    pub run_step: usize,
    /// Current retry counter for this specific tool.
    pub retry: usize,
    /// Maximum retries allowed for this specific tool.
    pub max_retries: usize,
    /// Tool call metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
    /// Typed dependencies, skipped from serialization.
    #[serde(skip)]
    pub dependencies: DependencyStore,
}

impl ToolContext {
    /// Build context for one tool call.
    #[must_use]
    pub fn new(run_id: RunId, conversation_id: ConversationId, run_step: usize) -> Self {
        Self {
            run_id,
            conversation_id,
            run_step,
            retry: 0,
            max_retries: 0,
            metadata: Metadata::default(),
            dependencies: DependencyStore::new(),
        }
    }

    /// Attach dependency store.
    #[must_use]
    pub fn with_dependencies(mut self, dependencies: DependencyStore) -> Self {
        self.dependencies = dependencies;
        self
    }

    /// Attach per-tool retry state.
    #[must_use]
    pub const fn with_retry_budget(mut self, retry: usize, max_retries: usize) -> Self {
        self.retry = retry;
        self.max_retries = max_retries;
        self
    }

    /// Return whether this execution is the final allowed attempt for this tool.
    #[must_use]
    pub const fn last_attempt(&self) -> bool {
        self.retry >= self.max_retries
    }

    /// Get a typed dependency.
    #[must_use]
    pub fn dependency<T>(&self) -> Option<Arc<T>>
    where
        T: Send + Sync + 'static,
    {
        self.dependencies.get::<T>()
    }

    /// Get a named typed dependency.
    #[must_use]
    pub fn named_dependency<T>(&self, name: &str) -> Option<Arc<T>>
    where
        T: Send + Sync + 'static,
    {
        self.dependencies.get_named::<T>(name)
    }
}
