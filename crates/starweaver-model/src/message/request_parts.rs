//! Canonical model request parts.

use serde::{Deserialize, Serialize};
use serde_json::Map;

use super::{Metadata, ToolReturnPart};

/// Cache-point lifetime for providers with per-breakpoint TTL controls.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum CachePointTtl {
    /// Keep the cache entry for at least five minutes.
    #[serde(rename = "5m")]
    FiveMinutes,
    /// Keep the cache entry for at least one hour.
    #[serde(rename = "1h")]
    OneHour,
}

impl CachePointTtl {
    /// Return the provider wire value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FiveMinutes => "5m",
            Self::OneHour => "1h",
        }
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
    /// Prompt-cache boundary after the preceding content block.
    ///
    /// The marker is not model-visible content. A TTL is optional because
    /// providers such as Anthropic use per-point `5m`/`1h` TTLs, while `OpenAI`
    /// GPT-5.6 uses a request-wide `30m` policy.
    CachePoint {
        /// Optional provider-compatible per-point TTL.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ttl: Option<CachePointTtl>,
    },
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

impl ContentPart {
    /// Build a provider-neutral prompt-cache boundary.
    #[must_use]
    pub const fn cache_point() -> Self {
        Self::CachePoint { ttl: None }
    }

    /// Build a prompt-cache boundary with a per-point TTL.
    #[must_use]
    pub const fn cache_point_with_ttl(ttl: CachePointTtl) -> Self {
        Self::CachePoint { ttl: Some(ttl) }
    }

    /// Build a plain text content part.
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }

    /// Build an image URL content part.
    #[must_use]
    pub fn image_url(url: impl Into<String>) -> Self {
        Self::ImageUrl { url: url.into() }
    }

    /// Build a generic file URL content part with an explicit media type.
    #[must_use]
    pub fn file_url(url: impl Into<String>, media_type: impl Into<String>) -> Self {
        Self::FileUrl {
            url: url.into(),
            media_type: media_type.into(),
        }
    }

    /// Build an inline binary content part with an explicit media type.
    #[must_use]
    pub fn binary(data: impl Into<Vec<u8>>, media_type: impl Into<String>) -> Self {
        Self::Binary {
            data: data.into(),
            media_type: media_type.into(),
        }
    }

    /// Build an inline image content part.
    #[must_use]
    pub fn image_bytes(data: impl Into<Vec<u8>>, media_type: impl Into<String>) -> Self {
        Self::binary(data, media_type)
    }

    /// Build an inline audio content part.
    #[must_use]
    pub fn audio_bytes(data: impl Into<Vec<u8>>, media_type: impl Into<String>) -> Self {
        Self::binary(data, media_type)
    }

    /// Build an inline video content part.
    #[must_use]
    pub fn video_bytes(data: impl Into<Vec<u8>>, media_type: impl Into<String>) -> Self {
        Self::binary(data, media_type)
    }

    /// Build an inline data URL content part with an explicit media type.
    #[must_use]
    pub fn data_url(data_url: impl Into<String>, media_type: impl Into<String>) -> Self {
        Self::DataUrl {
            data_url: data_url.into(),
            media_type: media_type.into(),
        }
    }

    /// Build a resource-backed media reference.
    #[must_use]
    pub fn resource_ref(
        uri: impl Into<String>,
        media_type: impl Into<String>,
        resource_type: impl Into<String>,
    ) -> Self {
        Self::ResourceRef {
            uri: uri.into(),
            media_type: media_type.into(),
            resource_type: resource_type.into(),
            metadata: Metadata::new(),
        }
    }

    /// Attach resource metadata to a resource-backed content part.
    #[must_use]
    pub fn with_resource_metadata(mut self, metadata: Metadata) -> Self {
        if let Self::ResourceRef {
            metadata: existing, ..
        } = &mut self
        {
            *existing = metadata;
        }
        self
    }
}
