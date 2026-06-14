//! Canonical model message history items.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Map;
use starweaver_core::{ConversationId, RunId, Usage};

use super::{
    ContentPart, FinishReason, Metadata, ModelRequestPart, ModelResponsePart, ProviderInfo,
    ToolCallPart,
};

/// Provider-neutral model history item.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ModelMessage {
    /// A request sent to a model.
    Request(ModelRequest),
    /// A response returned by a model.
    Response(ModelResponse),
}

/// Request item in canonical model history.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ModelRequest {
    /// Request parts sent in one model turn.
    pub parts: Vec<ModelRequestPart>,
    /// Creation timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<DateTime<Utc>>,
    /// Optional request-level instructions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    /// Run identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    /// Conversation identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<ConversationId>,
    /// Application metadata.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub metadata: Metadata,
}

impl ModelRequest {
    /// Build a user request from text.
    #[must_use]
    pub fn user_text(text: impl Into<String>) -> Self {
        Self {
            parts: vec![ModelRequestPart::UserPrompt {
                content: vec![ContentPart::Text { text: text.into() }],
                name: None,
                metadata: Metadata::default(),
            }],
            timestamp: None,
            instructions: None,
            run_id: None,
            conversation_id: None,
            metadata: Metadata::default(),
        }
    }
}

/// Response item in canonical model history.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ModelResponse {
    /// Response parts returned by the model.
    pub parts: Vec<ModelResponsePart>,
    /// Token and request usage.
    #[serde(default)]
    pub usage: Usage,
    /// Actual provider model name where known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_name: Option<String>,
    /// Provider metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<ProviderInfo>,
    /// Finish reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<FinishReason>,
    /// Response timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<DateTime<Utc>>,
    /// Run identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    /// Conversation identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<ConversationId>,
    /// Application metadata.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub metadata: Metadata,
}

impl ModelResponse {
    /// Build a text response.
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            parts: vec![ModelResponsePart::Text { text: text.into() }],
            usage: Usage::default(),
            model_name: None,
            provider: None,
            finish_reason: None,
            timestamp: None,
            run_id: None,
            conversation_id: None,
            metadata: Metadata::default(),
        }
    }

    /// Concatenate all text response parts.
    #[must_use]
    pub fn text_output(&self) -> String {
        self.parts
            .iter()
            .filter_map(ModelResponsePart::text)
            .collect::<Vec<_>>()
            .join("")
    }

    /// Return all provider-neutral tool calls in the response.
    #[must_use]
    pub fn tool_calls(&self) -> Vec<ToolCallPart> {
        self.parts
            .iter()
            .filter_map(ModelResponsePart::tool_call)
            .cloned()
            .collect()
    }
}
