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
    /// Inline binary media bytes.
    Binary {
        /// Binary payload.
        data: Vec<u8>,
        /// Declared or corrected media type.
        media_type: String,
    },
    /// Resource-backed media reference.
    ResourceRef {
        /// Resource URI.
        uri: String,
        /// Resource media type.
        media_type: String,
        /// Resource type, such as `image`, `video`, or `document`.
        resource_type: String,
        /// Resource metadata.
        #[serde(default, skip_serializing_if = "Map::is_empty")]
        metadata: Metadata,
    },
    /// Inline data URL for adapters that accept data URLs.
    DataUrl {
        /// Data URL payload.
        data_url: String,
        /// Media type carried by the data URL.
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
    /// Tool-call argument state.
    #[serde(default)]
    pub arguments: ToolArguments,
}

/// Tool-call argument state preserved across provider mapping, retries, and replay.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ToolArguments {
    /// Parsed JSON arguments ready for execution.
    Parsed(Value),
    /// Raw JSON string preserved for delayed parsing or provider wire replay.
    RawJsonString(String),
    /// Invalid JSON with original provider text and parser message.
    Invalid {
        /// Original provider argument text.
        raw: String,
        /// Parser error text.
        error: String,
    },
}

impl Default for ToolArguments {
    fn default() -> Self {
        Self::Parsed(Value::Null)
    }
}

impl ToolArguments {
    const INVALID_KIND: &'static str = "starweaver_invalid_tool_arguments";

    /// Build parsed JSON arguments.
    #[must_use]
    pub const fn parsed(value: Value) -> Self {
        Self::Parsed(value)
    }

    /// Build raw JSON string arguments.
    #[must_use]
    pub fn raw_json_string(raw: impl Into<String>) -> Self {
        Self::RawJsonString(raw.into())
    }

    /// Build invalid JSON arguments.
    #[must_use]
    pub fn invalid(raw: impl Into<String>, error: impl Into<String>) -> Self {
        Self::Invalid {
            raw: raw.into(),
            error: error.into(),
        }
    }

    /// Parse provider argument payload while preserving invalid input.
    #[must_use]
    pub fn from_provider_value(value: &Value) -> Self {
        match value {
            Value::String(raw) => match serde_json::from_str::<Value>(raw) {
                Ok(parsed) => Self::Parsed(parsed),
                Err(error) => Self::Invalid {
                    raw: raw.clone(),
                    error: error.to_string(),
                },
            },
            other => Self::Parsed(other.clone()),
        }
    }

    /// JSON value used for local tool execution and output functions.
    #[must_use]
    pub fn execution_value(&self) -> Value {
        match self {
            Self::Parsed(value) => value.clone(),
            Self::RawJsonString(raw) => {
                serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.clone()))
            }
            Self::Invalid { raw, .. } => Value::String(raw.clone()),
        }
    }

    /// JSON string used in provider wire requests.
    #[must_use]
    pub fn wire_json_string(&self) -> String {
        match self {
            Self::Parsed(value) => value.to_string(),
            Self::RawJsonString(raw) | Self::Invalid { raw, .. } => raw.clone(),
        }
    }

    /// Replay/display value carrying state evidence.
    #[must_use]
    pub fn replay_value(&self) -> Value {
        match self {
            Self::Parsed(value) => value.clone(),
            Self::RawJsonString(raw) => Value::String(raw.clone()),
            Self::Invalid { raw, error } => {
                let mut object = Map::new();
                object.insert(
                    "kind".to_string(),
                    Value::String(Self::INVALID_KIND.to_string()),
                );
                object.insert("raw".to_string(), Value::String(raw.clone()));
                object.insert("error".to_string(), Value::String(error.clone()));
                Value::Object(object)
            }
        }
    }

    /// Return the invalid parser error when present.
    #[must_use]
    pub fn invalid_error(&self) -> Option<&str> {
        match self {
            Self::Invalid { error, .. } => Some(error.as_str()),
            Self::Parsed(_) | Self::RawJsonString(_) => None,
        }
    }
}

impl From<Value> for ToolArguments {
    fn from(value: Value) -> Self {
        Self::Parsed(value)
    }
}

impl From<Map<String, Value>> for ToolArguments {
    fn from(value: Map<String, Value>) -> Self {
        Self::Parsed(Value::Object(value))
    }
}

impl From<&str> for ToolArguments {
    fn from(value: &str) -> Self {
        Self::RawJsonString(value.to_string())
    }
}

impl From<String> for ToolArguments {
    fn from(value: String) -> Self {
        Self::RawJsonString(value)
    }
}

impl PartialEq<Value> for ToolArguments {
    fn eq(&self, other: &Value) -> bool {
        &self.execution_value() == other
    }
}

impl PartialEq<ToolArguments> for Value {
    fn eq(&self, other: &ToolArguments) -> bool {
        self == &other.execution_value()
    }
}

impl Serialize for ToolArguments {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.replay_value().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ToolArguments {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        if let Value::Object(object) = &value {
            if object
                .get("kind")
                .and_then(Value::as_str)
                .is_some_and(|kind| kind == Self::INVALID_KIND)
            {
                return Ok(Self::Invalid {
                    raw: object
                        .get("raw")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    error: object
                        .get("error")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                });
            }
        }
        Ok(Self::Parsed(value))
    }
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
    /// Application-facing return value when it differs from model-visible content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_value: Option<Value>,
    /// User-facing content for UI display and host rendering.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_content: Option<Value>,
    /// Private host metadata kept separate from provider request mapping.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub private_metadata: Metadata,
}

impl ToolReturnPart {
    /// Build a tool return part with model-visible content.
    #[must_use]
    pub fn new(tool_call_id: impl Into<String>, name: impl Into<String>, content: Value) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            name: name.into(),
            content,
            is_error: false,
            metadata: Metadata::default(),
            app_value: None,
            user_content: None,
            private_metadata: Metadata::default(),
        }
    }

    /// Mark this tool return as an error.
    #[must_use]
    pub const fn with_error(mut self, is_error: bool) -> Self {
        self.is_error = is_error;
        self
    }

    /// Attach public runtime metadata.
    #[must_use]
    pub fn with_metadata(mut self, metadata: Metadata) -> Self {
        self.metadata = metadata;
        self
    }

    /// Attach application-facing return value.
    #[must_use]
    pub fn with_app_value(mut self, app_value: Value) -> Self {
        self.app_value = Some(app_value);
        self
    }

    /// Attach user-facing content.
    #[must_use]
    pub fn with_user_content(mut self, user_content: Value) -> Self {
        self.user_content = Some(user_content);
        self
    }

    /// Attach private host metadata.
    #[must_use]
    pub fn with_private_metadata(mut self, private_metadata: Metadata) -> Self {
        self.private_metadata = private_metadata;
        self
    }
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
