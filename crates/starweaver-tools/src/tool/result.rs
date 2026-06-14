use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use starweaver_core::Metadata;

use super::Tool;

/// Shared reference to a runtime tool.
pub type DynTool = Arc<dyn Tool>;

/// Result returned by a function tool.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolResult {
    /// JSON-serializable application return value.
    pub content: Value,
    /// Tool result metadata that can be recorded in model history and display streams.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
    /// Application-facing return value when it differs from `content`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_value: Option<Value>,
    /// Model-visible content override sent back through the provider tool-result protocol.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_content: Option<Value>,
    /// User-facing content override for UI display and host rendering.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_content: Option<Value>,
    /// Private host metadata kept away from model-visible tool content.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub private_metadata: Metadata,
}

impl ToolResult {
    /// Create a result from JSON content.
    #[must_use]
    pub fn new(content: Value) -> Self {
        Self {
            content,
            metadata: Metadata::default(),
            app_value: None,
            model_content: None,
            user_content: None,
            private_metadata: Metadata::default(),
        }
    }

    /// Attach runtime metadata to this result.
    #[must_use]
    pub fn with_metadata(mut self, metadata: Metadata) -> Self {
        self.metadata = metadata;
        self
    }

    /// Attach an application-facing return value.
    #[must_use]
    pub fn with_app_value(mut self, app_value: Value) -> Self {
        self.app_value = Some(app_value);
        self
    }

    /// Attach model-visible content that replaces `content` in provider tool returns.
    #[must_use]
    pub fn with_model_content(mut self, model_content: Value) -> Self {
        self.model_content = Some(model_content);
        self
    }

    /// Attach user-facing content for display and host rendering.
    #[must_use]
    pub fn with_user_content(mut self, user_content: Value) -> Self {
        self.user_content = Some(user_content);
        self
    }

    /// Attach private host metadata kept separate from model-visible content.
    #[must_use]
    pub fn with_private_metadata(mut self, private_metadata: Metadata) -> Self {
        self.private_metadata = private_metadata;
        self
    }
}

impl From<Value> for ToolResult {
    fn from(value: Value) -> Self {
        Self::new(value)
    }
}
