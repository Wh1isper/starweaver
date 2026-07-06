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
    /// Tool execution failed unexpectedly.
    #[error("tool {tool} failed: {message}")]
    Execution {
        /// Tool name.
        tool: String,
        /// Error message.
        message: String,
    },
    /// Tool was used incorrectly by application or integration code.
    #[error("tool {tool} user error: {message}")]
    UserError {
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
    /// Tool returned agent-readable feedback without treating it as a model or tool failure.
    #[error("tool {tool} returned feedback: {message}")]
    Feedback {
        /// Tool name.
        tool: String,
        /// Feedback message.
        message: String,
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
    /// Tool error carrying host-private metadata.
    #[error("{source}")]
    WithPrivateMetadata {
        /// Wrapped tool error.
        #[source]
        source: Box<Self>,
        /// Private metadata kept away from model-visible content.
        private_metadata: Metadata,
    },
}

/// Convert a tool error into a model-visible tool return.
#[must_use]
pub fn error_return(call: &ToolCallPart, error: &ToolError) -> ToolReturnPart {
    let (kind, mut metadata) = tool_error_metadata(error);
    let tool = error.tool_name().to_string();
    let message = error.user_message();
    let (how_to_fix, retryable, retry_requires_corrected_input) = error.recovery_guidance();
    let runtime_retryable = error.runtime_retryable();
    let unexpected = error.unexpected();

    metadata.insert("error_kind".to_string(), serde_json::json!(kind));
    metadata.insert("tool".to_string(), serde_json::json!(tool));
    metadata.insert("message".to_string(), serde_json::json!(message));
    metadata.insert("how_to_fix".to_string(), serde_json::json!(how_to_fix));
    metadata.insert("retryable".to_string(), serde_json::json!(retryable));
    metadata.insert(
        "retry_requires_corrected_input".to_string(),
        serde_json::json!(retry_requires_corrected_input),
    );
    metadata.insert(
        "runtime_retryable".to_string(),
        serde_json::json!(runtime_retryable),
    );
    metadata.insert(
        "model_retryable".to_string(),
        serde_json::json!(runtime_retryable),
    );
    metadata.insert("unexpected".to_string(), serde_json::json!(unexpected));

    ToolReturnPart {
        tool_call_id: call.id.clone(),
        name: call.name.clone(),
        content: serde_json::json!({
            "error": error.to_string(),
            "kind": kind,
            "tool": tool,
            "message": message,
            "how_to_fix": how_to_fix,
            "retryable": retryable,
            "retry_requires_corrected_input": retry_requires_corrected_input,
            "runtime_retryable": runtime_retryable,
            "model_retryable": runtime_retryable,
            "unexpected": unexpected,
            "success": false,
        }),
        is_error: error.is_error_return(),
        metadata,
        app_value: None,
        user_content: None,
        private_metadata: error.private_metadata(),
    }
}

impl ToolError {
    /// Attach host-private metadata to this error.
    #[must_use]
    pub fn with_private_metadata(self, private_metadata: Metadata) -> Self {
        if private_metadata.is_empty() {
            return self;
        }
        Self::WithPrivateMetadata {
            source: Box::new(self),
            private_metadata,
        }
    }

    /// Return host-private metadata attached to this error.
    #[must_use]
    pub fn private_metadata(&self) -> Metadata {
        match self {
            Self::WithPrivateMetadata {
                source,
                private_metadata,
            } => {
                let mut metadata = source.private_metadata();
                metadata.extend(private_metadata.clone());
                metadata
            }
            Self::NotFound(_)
            | Self::InvalidArguments { .. }
            | Self::Execution { .. }
            | Self::UserError { .. }
            | Self::Timeout { .. }
            | Self::Cancelled { .. }
            | Self::Feedback { .. }
            | Self::ModelRetry { .. }
            | Self::ApprovalRequired { .. }
            | Self::CallDeferred { .. } => Metadata::default(),
        }
    }

    fn tool_name(&self) -> &str {
        match self {
            Self::NotFound(tool)
            | Self::InvalidArguments { tool, .. }
            | Self::Execution { tool, .. }
            | Self::UserError { tool, .. }
            | Self::Timeout { tool, .. }
            | Self::Cancelled { tool, .. }
            | Self::Feedback { tool, .. }
            | Self::ModelRetry { tool, .. }
            | Self::ApprovalRequired { tool, .. }
            | Self::CallDeferred { tool, .. } => tool,
            Self::WithPrivateMetadata { source, .. } => source.tool_name(),
        }
    }

    fn user_message(&self) -> String {
        match self {
            Self::NotFound(tool) => format!("tool {tool:?} is not registered for this run"),
            Self::InvalidArguments { message, .. }
            | Self::Execution { message, .. }
            | Self::UserError { message, .. }
            | Self::Feedback { message, .. }
            | Self::ModelRetry { message, .. } => message.clone(),
            Self::Timeout { timeout_ms, .. } => {
                format!("tool execution exceeded the {timeout_ms}ms timeout")
            }
            Self::Cancelled { reason, .. } => reason.clone(),
            Self::ApprovalRequired { .. } => {
                "tool call requires approval before execution".to_string()
            }
            Self::CallDeferred { .. } => {
                "tool call was deferred to an external worker or later run".to_string()
            }
            Self::WithPrivateMetadata { source, .. } => source.user_message(),
        }
    }

    fn recovery_guidance(&self) -> (String, bool, bool) {
        match self {
            Self::NotFound(_) => (
                "Use one of the tools advertised in the current tool list. Check the exact tool name, namespace, and whether the tool is available in this agent context."
                    .to_string(),
                true,
                true,
            ),
            Self::InvalidArguments { .. } => (
                "Correct the tool arguments so they match the tool's JSON schema and retry the same tool call. Include required fields, use the documented field names and types, and avoid unsupported values."
                    .to_string(),
                true,
                true,
            ),
            Self::Execution { .. } => (
                "This looks like an unexpected tool/runtime failure. The runtime may retry the same tool call automatically. If it still fails, inspect the message and choose a safer alternative or report the tool/provider issue."
                    .to_string(),
                true,
                false,
            ),
            Self::UserError { .. } => (
                "This is an application or integration usage error, not a model-correctable tool failure. Fix the runtime/tool wiring, required dependencies, or call path before retrying."
                    .to_string(),
                false,
                false,
            ),
            Self::Feedback { message, .. } => (
                format!("Use this feedback to choose the next step; do not repeat the same call unchanged unless the underlying condition changed: {message}"),
                true,
                false,
            ),
            Self::Timeout { .. } => (
                "Retry with a larger timeout, reduce the amount of work, or run long-running work in background mode when the tool supports it."
                    .to_string(),
                true,
                true,
            ),
            Self::Cancelled { .. } => (
                "The owning agent run requested cancellation. Do not retry inside the cancelled run; resume or start a new run if work should continue."
                    .to_string(),
                false,
                false,
            ),
            Self::ModelRetry { message, .. } => (
                format!("Follow this correction request and call the tool again with adjusted arguments: {message}"),
                true,
                true,
            ),
            Self::ApprovalRequired { .. } => (
                "Wait for the approval decision. If approval is denied, change the plan or arguments before trying again."
                    .to_string(),
                false,
                false,
            ),
            Self::CallDeferred { .. } => (
                "Wait for the deferred result or use the runtime/session mechanism that resumes deferred tool calls. Do not immediately repeat the same call unless the deferral target changed."
                    .to_string(),
                false,
                false,
            ),
            Self::WithPrivateMetadata { source, .. } => source.recovery_guidance(),
        }
    }

    /// Return whether this error should trigger a model correction retry.
    #[must_use]
    pub fn runtime_retryable(&self) -> bool {
        match self {
            Self::InvalidArguments { .. } | Self::ModelRetry { .. } => true,
            Self::WithPrivateMetadata { source, .. } => source.runtime_retryable(),
            Self::NotFound(_)
            | Self::Execution { .. }
            | Self::UserError { .. }
            | Self::Timeout { .. }
            | Self::Cancelled { .. }
            | Self::Feedback { .. }
            | Self::ApprovalRequired { .. }
            | Self::CallDeferred { .. } => false,
        }
    }

    /// Return whether this error represents an unexpected tool/runtime failure.
    #[must_use]
    pub fn unexpected(&self) -> bool {
        match self {
            Self::Execution { .. } => true,
            Self::WithPrivateMetadata { source, .. } => source.unexpected(),
            Self::NotFound(_)
            | Self::InvalidArguments { .. }
            | Self::UserError { .. }
            | Self::Timeout { .. }
            | Self::Cancelled { .. }
            | Self::Feedback { .. }
            | Self::ModelRetry { .. }
            | Self::ApprovalRequired { .. }
            | Self::CallDeferred { .. } => false,
        }
    }

    fn is_error_return(&self) -> bool {
        match self {
            Self::InvalidArguments { .. }
            | Self::Execution { .. }
            | Self::UserError { .. }
            | Self::Timeout { .. }
            | Self::Cancelled { .. }
            | Self::ModelRetry { .. }
            | Self::ApprovalRequired { .. }
            | Self::CallDeferred { .. } => true,
            Self::WithPrivateMetadata { source, .. } => source.is_error_return(),
            Self::NotFound(_) | Self::Feedback { .. } => false,
        }
    }
}

fn tool_error_metadata(error: &ToolError) -> (&'static str, Metadata) {
    let mut metadata = Metadata::default();
    match error {
        ToolError::NotFound(_) => ("not_found", metadata),
        ToolError::InvalidArguments { .. } => ("invalid_arguments", metadata),
        ToolError::Execution { .. } => ("execution", metadata),
        ToolError::UserError { .. } => ("user_error", metadata),
        ToolError::Feedback { .. } => ("feedback", metadata),
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
        ToolError::WithPrivateMetadata { source, .. } => tool_error_metadata(source),
    }
}
