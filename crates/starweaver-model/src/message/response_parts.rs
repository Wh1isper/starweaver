//! Canonical model response parts.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{ProviderPartInfo, ToolCallPart};

/// Response part returned by a model.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ModelResponsePart {
    /// Plain text output.
    Text {
        /// Output text.
        text: String,
    },
    /// Plain text output with provider-private replay metadata.
    ProviderText {
        /// Output text.
        text: String,
        /// Provider replay metadata.
        #[serde(default, skip_serializing_if = "ProviderPartInfo::is_empty")]
        provider: ProviderPartInfo,
    },
    /// Reasoning or thinking output.
    Thinking {
        /// Thinking text.
        text: String,
        /// Provider signature where available.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    /// Reasoning or thinking output with provider-private replay metadata.
    ProviderThinking {
        /// Thinking text or provider-provided reasoning summary.
        text: String,
        /// Provider signature or encrypted reasoning payload where available.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
        /// Provider replay metadata.
        #[serde(default, skip_serializing_if = "ProviderPartInfo::is_empty")]
        provider: ProviderPartInfo,
    },
    /// Provider-neutral tool call.
    ToolCall(ToolCallPart),
    /// Provider-neutral tool call with provider-private replay metadata.
    ProviderToolCall {
        /// Tool call visible to runtime tool execution.
        call: ToolCallPart,
        /// Provider replay metadata, including output item ID and namespaces.
        #[serde(default, skip_serializing_if = "ProviderPartInfo::is_empty")]
        provider: ProviderPartInfo,
    },
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
    /// Opaque provider output item kept for same-provider replay.
    ProviderOpaque {
        /// Provider output item type.
        item_type: String,
        /// Provider payload.
        payload: Value,
        /// Provider replay metadata.
        #[serde(default, skip_serializing_if = "ProviderPartInfo::is_empty")]
        provider: ProviderPartInfo,
    },
}

impl ModelResponsePart {
    /// Return text content for text-like parts.
    #[must_use]
    pub const fn text(&self) -> Option<&str> {
        match self {
            Self::Text { text } | Self::ProviderText { text, .. } => Some(text.as_str()),
            Self::Thinking { .. }
            | Self::ProviderThinking { .. }
            | Self::ToolCall(_)
            | Self::ProviderToolCall { .. }
            | Self::NativeToolCall { .. }
            | Self::NativeToolReturn { .. }
            | Self::File { .. }
            | Self::Compaction { .. }
            | Self::ProviderOpaque { .. } => None,
        }
    }

    /// Return thinking text and signature for reasoning-like parts.
    #[must_use]
    pub fn thinking(&self) -> Option<(&str, Option<&str>)> {
        match self {
            Self::Thinking { text, signature }
            | Self::ProviderThinking {
                text, signature, ..
            } => Some((text.as_str(), signature.as_deref())),
            Self::Text { .. }
            | Self::ProviderText { .. }
            | Self::ToolCall(_)
            | Self::ProviderToolCall { .. }
            | Self::NativeToolCall { .. }
            | Self::NativeToolReturn { .. }
            | Self::File { .. }
            | Self::Compaction { .. }
            | Self::ProviderOpaque { .. } => None,
        }
    }

    /// Return a function-style tool call when present.
    #[must_use]
    pub const fn tool_call(&self) -> Option<&ToolCallPart> {
        match self {
            Self::ToolCall(call) | Self::ProviderToolCall { call, .. } => Some(call),
            Self::Text { .. }
            | Self::ProviderText { .. }
            | Self::Thinking { .. }
            | Self::ProviderThinking { .. }
            | Self::NativeToolCall { .. }
            | Self::NativeToolReturn { .. }
            | Self::File { .. }
            | Self::Compaction { .. }
            | Self::ProviderOpaque { .. } => None,
        }
    }

    /// Return provider replay metadata when present.
    #[must_use]
    pub const fn provider_part(&self) -> Option<&ProviderPartInfo> {
        match self {
            Self::ProviderText { provider, .. }
            | Self::ProviderThinking { provider, .. }
            | Self::ProviderToolCall { provider, .. }
            | Self::ProviderOpaque { provider, .. } => Some(provider),
            Self::Text { .. }
            | Self::Thinking { .. }
            | Self::ToolCall(_)
            | Self::NativeToolCall { .. }
            | Self::NativeToolReturn { .. }
            | Self::File { .. }
            | Self::Compaction { .. } => None,
        }
    }

    /// Return true when this response part is a compaction boundary.
    #[must_use]
    pub fn is_compaction(&self) -> bool {
        match self {
            Self::Compaction { .. } => true,
            Self::ProviderOpaque { item_type, .. } => item_type == "compaction",
            Self::Text { .. }
            | Self::ProviderText { .. }
            | Self::Thinking { .. }
            | Self::ProviderThinking { .. }
            | Self::ToolCall(_)
            | Self::ProviderToolCall { .. }
            | Self::NativeToolCall { .. }
            | Self::NativeToolReturn { .. }
            | Self::File { .. } => false,
        }
    }
}
