//! Tool errors and error-to-tool-return mapping.

use serde_json::Value;
use starweaver_core::Metadata;
use starweaver_model::{ToolCallPart, ToolReturnPart};
use thiserror::Error;

/// Function tool execution error.
#[derive(Clone, Debug, Error)]
pub enum ToolError {
    /// Tool was not found.
    #[error("tool not found: {0}")]
    NotFound(String),
    /// Tool input failed validation.
    #[error("invalid tool arguments for {tool}: {message}")]
    InvalidArguments {
        /// Tool name.
        tool: String,
        /// Validation message.
        message: String,
    },
    /// Tool execution failed.
    #[error("tool {tool} failed: {message}")]
    Execution {
        /// Tool name.
        tool: String,
        /// Error message.
        message: String,
    },
    /// Tool execution exceeded its timeout.
    #[error("tool {tool} timed out after {timeout_ms}ms")]
    Timeout {
        /// Tool name.
        tool: String,
        /// Timeout in milliseconds.
        timeout_ms: u64,
    },
    /// Tool execution was cancelled by the owning run.
    #[error("tool {tool} cancelled: {reason}")]
    Cancelled {
        /// Tool name.
        tool: String,
        /// Cancellation reason.
        reason: String,
    },
    /// Tool asked the model to retry the call with corrected input.
    #[error("tool {tool} requested model retry: {message}")]
    ModelRetry {
        /// Tool name.
        tool: String,
        /// Retry prompt message.
        message: String,
    },
    /// Tool requires user approval.
    #[error("tool {tool} requires approval")]
    ApprovalRequired {
        /// Tool name.
        tool: String,
        /// Approval metadata.
        metadata: Value,
    },
    /// Tool call is deferred to another runtime.
    #[error("tool {tool} call deferred")]
    CallDeferred {
        /// Tool name.
        tool: String,
        /// Deferred-call metadata.
        metadata: Value,
    },
}

/// Convert a tool error into a model-visible tool return.
#[must_use]
pub fn error_return(call: &ToolCallPart, error: &ToolError) -> ToolReturnPart {
    let (kind, mut metadata) = tool_error_metadata(error);
    metadata.insert("error_kind".to_string(), serde_json::json!(kind));
    ToolReturnPart {
        tool_call_id: call.id.clone(),
        name: call.name.clone(),
        content: serde_json::json!({
            "error": error.to_string(),
            "kind": kind,
        }),
        is_error: true,
        metadata,
        app_value: None,
        user_content: None,
        private_metadata: Metadata::default(),
    }
}

fn tool_error_metadata(error: &ToolError) -> (&'static str, Metadata) {
    let mut metadata = Metadata::default();
    match error {
        ToolError::NotFound(_) => ("not_found", metadata),
        ToolError::InvalidArguments { .. } => ("invalid_arguments", metadata),
        ToolError::Execution { .. } => ("execution", metadata),
        ToolError::Timeout { timeout_ms, .. } => {
            metadata.insert("timeout_ms".to_string(), serde_json::json!(timeout_ms));
            ("timeout", metadata)
        }
        ToolError::Cancelled { .. } => ("cancelled", metadata),
        ToolError::ModelRetry { .. } => ("model_retry", metadata),
        ToolError::ApprovalRequired {
            metadata: value, ..
        } => {
            metadata.insert(
                "control_flow".to_string(),
                serde_json::json!("approval_required"),
            );
            metadata.insert("approval".to_string(), value.clone());
            ("approval_required", metadata)
        }
        ToolError::CallDeferred {
            metadata: value, ..
        } => {
            metadata.insert(
                "control_flow".to_string(),
                serde_json::json!("call_deferred"),
            );
            metadata.insert("deferred".to_string(), value.clone());
            ("call_deferred", metadata)
        }
    }
}
