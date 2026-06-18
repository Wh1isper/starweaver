//! Tool execution middleware.

use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use serde_json::Value;
use starweaver_model::ToolCallPart;

use crate::{ToolContext, ToolError, ToolResult};

/// Shared tool execution middleware reference.
pub type DynToolExecutionHook = Arc<dyn ToolExecutionHook>;

/// Mutable result of a local tool execution before it is converted into a model tool return.
#[derive(Clone, Debug)]
pub enum ToolExecutionOutcome {
    /// Tool returned successfully.
    Success(ToolResult),
    /// Tool returned a structured execution error.
    Error(ToolError),
}

impl ToolExecutionOutcome {
    /// Build an outcome from a tool result.
    #[must_use]
    pub fn from_result(result: Result<ToolResult, ToolError>) -> Self {
        match result {
            Ok(result) => Self::Success(result),
            Err(error) => Self::Error(error),
        }
    }

    /// Return whether this outcome carries approval/deferred control flow.
    #[must_use]
    pub const fn is_control_flow(&self) -> bool {
        matches!(
            self,
            Self::Error(ToolError::ApprovalRequired { .. } | ToolError::CallDeferred { .. })
        )
    }

    /// Convert this outcome back into a tool result.
    ///
    /// # Errors
    ///
    /// Returns the contained tool error when this outcome is [`Self::Error`].
    pub fn into_result(self) -> Result<ToolResult, ToolError> {
        match self {
            Self::Success(result) => Ok(result),
            Self::Error(error) => Err(error),
        }
    }
}

/// Hook that can observe or transform one local tool call.
#[async_trait]
pub trait ToolExecutionHook: Send + Sync {
    /// Called before the tool function runs.
    ///
    /// Hooks can mutate the execution context and arguments. Returning a `ToolError` skips the
    /// tool call and turns that error into the model-visible tool return.
    async fn before_tool_call(
        &self,
        _context: &mut ToolContext,
        _call: &ToolCallPart,
        _arguments: &mut Value,
    ) -> Result<(), ToolError> {
        Ok(())
    }

    /// Called after the tool function returns.
    ///
    /// Hooks can mutate ordinary success/error outcomes. Approval and deferred-control-flow
    /// outcomes are passed to post hooks for observation only by `ToolExecutionHooks`.
    async fn after_tool_call(
        &self,
        _context: &ToolContext,
        _call: &ToolCallPart,
        _outcome: &mut ToolExecutionOutcome,
    ) -> Result<(), ToolError> {
        Ok(())
    }
}

/// Ordered middleware applied around local tool execution.
#[derive(Clone, Default)]
pub struct ToolExecutionHooks {
    global_hooks: Vec<DynToolExecutionHook>,
    tool_hooks: BTreeMap<String, Vec<DynToolExecutionHook>>,
}

impl ToolExecutionHooks {
    /// Create an empty hook collection.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Return whether no hooks are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.global_hooks.is_empty() && self.tool_hooks.is_empty()
    }

    /// Add a global hook.
    #[must_use]
    pub fn with_global_hook(mut self, hook: DynToolExecutionHook) -> Self {
        self.push_global_hook(hook);
        self
    }

    /// Add a hook for a single tool name.
    #[must_use]
    pub fn with_tool_hook(mut self, tool: impl Into<String>, hook: DynToolExecutionHook) -> Self {
        self.push_tool_hook(tool, hook);
        self
    }

    /// Append a global hook.
    pub fn push_global_hook(&mut self, hook: DynToolExecutionHook) {
        self.global_hooks.push(hook);
    }

    /// Append a hook for a single tool name.
    pub fn push_tool_hook(&mut self, tool: impl Into<String>, hook: DynToolExecutionHook) {
        self.tool_hooks.entry(tool.into()).or_default().push(hook);
    }

    /// Extend this collection with hooks from another collection.
    pub fn extend(&mut self, other: &Self) {
        self.global_hooks.extend(other.global_hooks.iter().cloned());
        for (name, hooks) in &other.tool_hooks {
            self.tool_hooks
                .entry(name.clone())
                .or_default()
                .extend(hooks.iter().cloned());
        }
    }

    /// Return a copy with global hooks and hooks for the selected tool names.
    #[must_use]
    pub fn select_for_tools<'a>(&self, names: impl IntoIterator<Item = &'a str>) -> Self {
        let mut selected = Self {
            global_hooks: self.global_hooks.clone(),
            tool_hooks: BTreeMap::new(),
        };
        for name in names {
            if let Some(hooks) = self.tool_hooks.get(name) {
                selected.tool_hooks.insert(name.to_string(), hooks.clone());
            }
        }
        selected
    }

    /// Run pre-execution hooks in global then per-tool order.
    ///
    /// # Errors
    ///
    /// Returns the first hook error.
    pub async fn run_before(
        &self,
        context: &mut ToolContext,
        call: &ToolCallPart,
        arguments: &mut Value,
    ) -> Result<(), ToolError> {
        for hook in &self.global_hooks {
            hook.before_tool_call(context, call, arguments).await?;
        }
        if let Some(hooks) = self.tool_hooks.get(&call.name) {
            for hook in hooks {
                hook.before_tool_call(context, call, arguments).await?;
            }
        }
        Ok(())
    }

    /// Run post-execution hooks in per-tool then global order.
    ///
    /// Approval and deferred-control-flow outcomes are observed through a cloned outcome and the
    /// original outcome is preserved.
    ///
    /// # Errors
    ///
    /// Returns the first hook error for ordinary outcomes. Control-flow outcomes keep the original
    /// approval/deferred result.
    pub async fn run_after(
        &self,
        context: &ToolContext,
        call: &ToolCallPart,
        outcome: &mut ToolExecutionOutcome,
    ) -> Result<(), ToolError> {
        if outcome.is_control_flow() {
            let mut observed = outcome.clone();
            self.run_after_mutating(context, call, &mut observed)
                .await
                .ok();
            return Ok(());
        }
        self.run_after_mutating(context, call, outcome).await
    }

    async fn run_after_mutating(
        &self,
        context: &ToolContext,
        call: &ToolCallPart,
        outcome: &mut ToolExecutionOutcome,
    ) -> Result<(), ToolError> {
        if let Some(hooks) = self.tool_hooks.get(&call.name) {
            for hook in hooks {
                hook.after_tool_call(context, call, outcome).await?;
            }
        }
        for hook in &self.global_hooks {
            hook.after_tool_call(context, call, outcome).await?;
        }
        Ok(())
    }
}
