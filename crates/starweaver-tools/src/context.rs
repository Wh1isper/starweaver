//! Tool execution context.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use starweaver_context::{DependencyStore, NoteStore, StateStore};
use starweaver_core::{ConversationId, Metadata, RunId, TraceContext};

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
    /// Serializable application state snapshot.
    #[serde(default)]
    pub state: StateStore,
    /// Serializable note snapshot.
    #[serde(default, skip_serializing_if = "NoteStore::is_empty")]
    pub notes: NoteStore,
    /// Trace correlation context propagated from the runtime context.
    #[serde(default, skip_serializing_if = "TraceContext::is_empty")]
    pub trace_context: TraceContext,
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
            state: StateStore::new(),
            notes: NoteStore::new(),
            trace_context: TraceContext::default(),
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

    /// Attach serializable state snapshot.
    #[must_use]
    pub fn with_state(mut self, state: StateStore) -> Self {
        self.state = state;
        self
    }

    /// Attach serializable note snapshot.
    #[must_use]
    pub fn with_notes(mut self, notes: NoteStore) -> Self {
        self.notes = notes;
        self
    }

    /// Attach trace correlation context.
    #[must_use]
    pub fn with_trace_context(mut self, trace_context: TraceContext) -> Self {
        self.trace_context = trace_context;
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
