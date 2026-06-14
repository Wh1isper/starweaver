//! Canonical model request parts.

use serde::{Deserialize, Serialize};
use serde_json::Map;

use super::{Metadata, ToolReturnPart};

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
