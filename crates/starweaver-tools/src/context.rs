//! Tool execution context.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use starweaver_context::DependencyStore;
use starweaver_core::{ConversationId, Metadata, RunId, TraceContext};

/// Inline approval state attached by runtime capability hooks.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ToolApprovalState {
    /// Tool execution was approved.
    Approved {
        /// Optional replacement arguments.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        override_arguments: Option<serde_json::Value>,
        /// Approval metadata.
        #[serde(default, skip_serializing_if = "Metadata::is_empty")]
        metadata: Metadata,
    },
    /// Tool execution was denied.
    Denied {
        /// Denial reason.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        /// Denial metadata.
        #[serde(default, skip_serializing_if = "Metadata::is_empty")]
        metadata: Metadata,
    },
}

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
    /// Trace correlation context propagated from the runtime context.
    #[serde(default, skip_serializing_if = "TraceContext::is_empty")]
    pub trace_context: TraceContext,
    /// Tool call metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
    /// Inline approval decision set by runtime capability hooks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<ToolApprovalState>,
    /// Inline deferred result supplied by runtime capability hooks or hosts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deferred_result: Option<serde_json::Value>,
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
            trace_context: TraceContext::default(),
            metadata: Metadata::default(),
            approval: None,
            deferred_result: None,
            dependencies: DependencyStore::new(),
        }
    }

    /// Attach dependency store.
    #[must_use]
    pub fn with_dependencies(mut self, dependencies: DependencyStore) -> Self {
        self.dependencies = dependencies;
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

    /// Attach inline approval state.
    #[must_use]
    pub fn with_approval(mut self, approval: ToolApprovalState) -> Self {
        self.approval = Some(approval);
        self
    }

    /// Mark this tool call as approved.
    pub fn approve(&mut self) {
        self.approval = Some(ToolApprovalState::Approved {
            override_arguments: None,
            metadata: Metadata::default(),
        });
    }

    /// Mark this tool call as approved with replacement arguments.
    pub fn approve_with_arguments(&mut self, arguments: serde_json::Value) {
        self.approval = Some(ToolApprovalState::Approved {
            override_arguments: Some(arguments),
            metadata: Metadata::default(),
        });
    }

    /// Mark this tool call as denied.
    pub fn deny(&mut self, reason: impl Into<String>) {
        self.approval = Some(ToolApprovalState::Denied {
            reason: Some(reason.into()),
            metadata: Metadata::default(),
        });
    }

    /// Attach inline deferred result content.
    #[must_use]
    pub fn with_deferred_result(mut self, result: serde_json::Value) -> Self {
        self.deferred_result = Some(result);
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
