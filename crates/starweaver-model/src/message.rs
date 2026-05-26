//! Canonical message history and request/response parts.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use starweaver_core::{ConversationId, RunId, Usage};

/// Serializable metadata object used by model messages and parts.
pub type Metadata = Map<String, Value>;

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
            .filter_map(|part| match part {
                ModelResponsePart::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }

    /// Return all provider-neutral tool calls in the response.
    #[must_use]
    pub fn tool_calls(&self) -> Vec<ToolCallPart> {
        self.parts
            .iter()
            .filter_map(|part| match part {
                ModelResponsePart::ToolCall(call) => Some(call.clone()),
                _ => None,
            })
            .collect()
    }
}

/// Request part sent to a model.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ModelRequestPart {
    /// System or developer instruction.
    SystemPrompt {
        /// Prompt text.
        text: String,
        /// Application metadata.
        #[serde(default, skip_serializing_if = "Map::is_empty")]
        metadata: Metadata,
    },
    /// User prompt.
    UserPrompt {
        /// User content parts.
        content: Vec<ContentPart>,
        /// Optional speaker name.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        /// Application metadata.
        #[serde(default, skip_serializing_if = "Map::is_empty")]
        metadata: Metadata,
    },
    /// Tool return sent back to the model.
    ToolReturn(ToolReturnPart),
    /// Retry or validation feedback.
    RetryPrompt {
        /// Feedback text.
        text: String,
        /// Related tool call identifier.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_call_id: Option<String>,
        /// Application metadata.
        #[serde(default, skip_serializing_if = "Map::is_empty")]
        metadata: Metadata,
    },
    /// Structured instruction fragment.
    Instruction {
        /// Instruction text.
        text: String,
        /// Application metadata.
        #[serde(default, skip_serializing_if = "Map::is_empty")]
        metadata: Metadata,
    },
}

/// Multimodal content in user requests.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ContentPart {
    /// Plain text.
    Text {
        /// Text content.
        text: String,
    },
    /// Image by URL.
    ImageUrl {
        /// Image URL.
        url: String,
    },
    /// Generic file by URL and media type.
    FileUrl {
        /// File URL.
        url: String,
        /// File media type.
        media_type: String,
    },
}

/// Response part returned by a model.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ModelResponsePart {
    /// Plain text output.
    Text {
        /// Output text.
        text: String,
    },
    /// Reasoning or thinking output.
    Thinking {
        /// Thinking text.
        text: String,
        /// Provider signature where available.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    /// Provider-neutral tool call.
    ToolCall(ToolCallPart),
    /// Provider-native tool call payload.
    NativeToolCall {
        /// Native tool type.
        tool_type: String,
        /// Provider payload.
        payload: Value,
    },
    /// Provider-native tool return payload.
    NativeToolReturn {
        /// Native tool type.
        tool_type: String,
        /// Provider payload.
        payload: Value,
    },
    /// File returned by the model.
    File {
        /// File URL.
        url: String,
        /// File media type.
        media_type: String,
    },
    /// Compaction summary.
    Compaction {
        /// Summary text.
        summary: String,
    },
}

/// Function-style tool call.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolCallPart {
    /// Provider or runtime call identifier.
    pub id: String,
    /// Tool name.
    pub name: String,
    /// JSON object arguments.
    #[serde(default)]
    pub arguments: Value,
}

/// Tool return content.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolReturnPart {
    /// Related call identifier.
    pub tool_call_id: String,
    /// Tool name.
    pub name: String,
    /// Tool content sent back to the model.
    pub content: Value,
    /// Tool result status.
    #[serde(default)]
    pub is_error: bool,
    /// Tool return metadata for approval, deferral, and runtime orchestration.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub metadata: Metadata,
}

/// Provider metadata attached to canonical responses.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderInfo {
    /// Provider name.
    pub name: String,
    /// Provider response identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_id: Option<String>,
}

/// Provider-neutral finish reason.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    /// Natural stop.
    Stop,
    /// Length or token limit.
    Length,
    /// Tool calls requested.
    ToolCalls,
    /// Content filtered by provider.
    ContentFilter,
    /// Unknown provider reason.
    Unknown,
}
