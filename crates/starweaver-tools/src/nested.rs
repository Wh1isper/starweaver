use std::{collections::BTreeSet, future::Future, sync::Arc};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};

/// Metadata marker for an orchestration tool that requires the runtime-owned nested invoker.
pub const TOOL_METADATA_NESTED_INVOKER_KEY: &str = "starweaver_nested_tool_invoker";
/// Metadata key carrying the host-enforced maximum child-call count for one orchestration call.
pub const TOOL_METADATA_NESTED_CALL_LIMIT_KEY: &str = "starweaver_nested_tool_call_limit";
/// Metadata key carrying the maximum public terminal child-result evidence size.
pub const TOOL_METADATA_NESTED_RESULT_MAX_BYTES_KEY: &str = "starweaver_nested_result_max_bytes";
/// Result metadata marker for a failed child effect that constrained code must not resume past.
pub const TOOL_RESULT_NESTED_NON_RESUMABLE_KEY: &str = "starweaver_nested_non_resumable_effect";

/// A request submitted by a constrained orchestration tool to the active runtime broker.
#[derive(Debug)]
pub struct NestedToolRequest {
    /// Exact canonical target name from the prepared run-step registry.
    pub tool_name: String,
    /// JSON arguments for the target tool.
    pub arguments: Value,
    /// One-shot response channel owned by the caller.
    pub response: oneshot::Sender<Result<NestedToolResult, NestedToolError>>,
}

/// Public, bounded result projection returned to constrained code.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NestedToolResult {
    /// Public model-safe result content.
    pub content: Value,
    /// Public metadata explicitly retained by the broker.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub metadata: serde_json::Map<String, Value>,
}

/// Typed nested invocation failure.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, thiserror::Error)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NestedToolError {
    /// The target is not in the execution's pinned allowlist.
    #[error("nested tool target is not allowed: {tool_name}")]
    NotAllowed {
        /// Rejected canonical target.
        tool_name: String,
    },
    /// The execution exhausted its child-call budget.
    #[error("nested tool call budget exhausted")]
    CallLimit,
    /// The runtime broker is no longer available.
    #[error("nested tool broker is unavailable")]
    BrokerUnavailable,
    /// The nested call exceeded the orchestration execution deadline.
    #[error("nested tool call exceeded the execution deadline")]
    DeadlineExceeded,
    /// The nested call was cancelled.
    #[error("nested tool call was cancelled")]
    Cancelled,
    /// The target requested control flow that cannot resume constrained source.
    #[error("nested tool control flow cannot be resumed: {message}")]
    NonResumableControlFlow {
        /// Safe public explanation.
        message: String,
    },
    /// The target failed through its canonical execution path.
    #[error("nested tool failed: {message}")]
    ToolFailed {
        /// Safe public explanation.
        message: String,
    },
}

/// Execution-scoped handle injected by the runtime into an admitted orchestration tool.
#[derive(Clone)]
pub struct NestedToolInvoker {
    sender: mpsc::Sender<NestedToolRequest>,
    allowed_tools: Arc<BTreeSet<String>>,
    deadline: Option<tokio::time::Instant>,
    cancellation_token: starweaver_core::CancellationToken,
}

impl std::fmt::Debug for NestedToolInvoker {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NestedToolInvoker")
            .field("allowed_tools", &self.allowed_tools)
            .finish_non_exhaustive()
    }
}

impl NestedToolInvoker {
    /// Build an execution-scoped invoker and return its runtime-owned receiver.
    #[must_use]
    pub fn channel(
        allowed_tools: BTreeSet<String>,
        capacity: usize,
    ) -> (Self, mpsc::Receiver<NestedToolRequest>) {
        let (sender, receiver) = mpsc::channel(capacity.max(1));
        (
            Self {
                sender,
                allowed_tools: Arc::new(allowed_tools),
                deadline: None,
                cancellation_token: starweaver_core::CancellationToken::default(),
            },
            receiver,
        )
    }

    /// Bind an absolute deadline and cooperative cancellation token to this execution handle.
    #[must_use]
    pub fn with_execution_control(
        mut self,
        deadline: tokio::time::Instant,
        cancellation_token: starweaver_core::CancellationToken,
    ) -> Self {
        self.deadline = Some(deadline);
        self.cancellation_token = cancellation_token;
        self
    }

    /// Return the exact pinned targets visible to this execution.
    #[must_use]
    pub fn allowed_tools(&self) -> &BTreeSet<String> {
        &self.allowed_tools
    }

    /// Return a child handle restricted to the intersection with `allowed_tools`.
    #[must_use]
    pub fn restricted_to(&self, allowed_tools: &BTreeSet<String>) -> Self {
        let allowed_tools = self
            .allowed_tools
            .intersection(allowed_tools)
            .cloned()
            .collect();
        Self {
            sender: self.sender.clone(),
            allowed_tools: Arc::new(allowed_tools),
            deadline: self.deadline,
            cancellation_token: self.cancellation_token.clone(),
        }
    }

    /// Invoke one exact target through the runtime-owned broker.
    ///
    /// # Errors
    ///
    /// Returns a typed error when the target is denied, the broker is gone, or execution fails.
    pub async fn invoke(
        &self,
        tool_name: impl Into<String>,
        arguments: Value,
    ) -> Result<NestedToolResult, NestedToolError> {
        let tool_name = tool_name.into();
        if !self.allowed_tools.contains(&tool_name) {
            return Err(NestedToolError::NotAllowed { tool_name });
        }
        let (response, result) = oneshot::channel();
        self.await_controlled(self.sender.send(NestedToolRequest {
            tool_name,
            arguments,
            response,
        }))
        .await?
        .map_err(|_| NestedToolError::BrokerUnavailable)?;
        self.await_controlled(result)
            .await?
            .map_err(|_| NestedToolError::BrokerUnavailable)?
    }

    async fn await_controlled<F, T>(&self, future: F) -> Result<T, NestedToolError>
    where
        F: Future<Output = T>,
    {
        if let Some(deadline) = self.deadline {
            tokio::select! {
                biased;
                () = self.cancellation_token.cancelled() => Err(NestedToolError::Cancelled),
                () = tokio::time::sleep_until(deadline) => Err(NestedToolError::DeadlineExceeded),
                output = future => Ok(output),
            }
        } else {
            tokio::select! {
                biased;
                () = self.cancellation_token.cancelled() => Err(NestedToolError::Cancelled),
                output = future => Ok(output),
            }
        }
    }
}

/// Executor-facing abstraction implemented by [`NestedToolInvoker`].
#[async_trait]
pub trait CodeToolBridge: Send + Sync {
    /// Invoke one canonical target.
    async fn call(&self, name: &str, arguments: Value)
    -> Result<NestedToolResult, NestedToolError>;
}

#[async_trait]
impl CodeToolBridge for NestedToolInvoker {
    async fn call(
        &self,
        name: &str,
        arguments: Value,
    ) -> Result<NestedToolResult, NestedToolError> {
        self.invoke(name, arguments).await
    }
}
