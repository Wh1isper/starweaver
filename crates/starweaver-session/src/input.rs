//! Versioned durable input part records.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use starweaver_core::Metadata;
use starweaver_model::{CachePointTtl, ContentPart};
use thiserror::Error;

/// Provider-scoped legacy file reference submitted as session input.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileRef {
    /// File URI or provider reference.
    pub uri: String,
    /// Optional media type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    /// Optional display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl FileRef {
    /// Build a file reference.
    #[must_use]
    pub fn new(uri: impl Into<String>) -> Self {
        Self {
            uri: uri.into(),
            media_type: None,
            name: None,
        }
    }
}

/// Binary resource legacy reference submitted as session input.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BinaryRef {
    /// Resource URI, object key, or upload token.
    pub uri: String,
    /// Optional media type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    /// Optional byte length.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
}

impl BinaryRef {
    /// Build a binary reference.
    #[must_use]
    pub fn new(uri: impl Into<String>) -> Self {
        Self {
            uri: uri.into(),
            media_type: None,
            bytes: None,
        }
    }
}

/// Serializable run input submitted by SDKs and product hosts.
///
/// Every canonical model [`ContentPart`] has an explicit durable variant. The
/// legacy `url`, `file`, `binary`, `mode`, and `command` variants remain readable
/// for previous-release evidence but are not used as a content escape hatch.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InputPart {
    /// Prompt-cache boundary after the preceding content part.
    CachePoint {
        /// Optional provider-compatible cache lifetime.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ttl: Option<CachePointTtl>,
        /// Application metadata.
        #[serde(default, skip_serializing_if = "Metadata::is_empty")]
        metadata: Metadata,
    },
    /// Natural language prompt text.
    Text {
        /// Text content.
        text: String,
        /// Application metadata.
        #[serde(default, skip_serializing_if = "Metadata::is_empty")]
        metadata: Metadata,
    },
    /// Image URL.
    ImageUrl {
        /// Image URL.
        url: String,
        /// Application metadata.
        #[serde(default, skip_serializing_if = "Metadata::is_empty")]
        metadata: Metadata,
    },
    /// Generic file URL with an explicit media type.
    FileUrl {
        /// File URL.
        url: String,
        /// File media type.
        media_type: String,
        /// Application metadata.
        #[serde(default, skip_serializing_if = "Metadata::is_empty")]
        metadata: Metadata,
    },
    /// Inline binary content.
    InlineBinary {
        /// Binary bytes.
        data: Vec<u8>,
        /// Declared media type.
        media_type: String,
        /// Application metadata.
        #[serde(default, skip_serializing_if = "Metadata::is_empty")]
        metadata: Metadata,
    },
    /// Resource-backed content reference.
    ResourceRef {
        /// Resource URI.
        uri: String,
        /// Resource media type.
        media_type: String,
        /// Resource type such as `image`, `video`, or `document`.
        resource_type: String,
        /// Resource metadata preserved for model preparation.
        #[serde(default, skip_serializing_if = "Metadata::is_empty")]
        resource_metadata: Metadata,
        /// Application metadata that is not model-visible by default.
        #[serde(default, skip_serializing_if = "Metadata::is_empty")]
        metadata: Metadata,
    },
    /// Inline data URL.
    DataUrl {
        /// Data URL payload.
        data_url: String,
        /// Media type carried by the data URL.
        media_type: String,
        /// Application metadata.
        #[serde(default, skip_serializing_if = "Metadata::is_empty")]
        metadata: Metadata,
    },
    /// Legacy generic URL reference. New content writers use `image_url` or `file_url`.
    Url {
        /// URL string.
        url: String,
        /// Application metadata.
        #[serde(default, skip_serializing_if = "Metadata::is_empty")]
        metadata: Metadata,
    },
    /// Legacy provider-scoped file reference.
    File {
        /// File reference.
        file: FileRef,
        /// Application metadata.
        #[serde(default, skip_serializing_if = "Metadata::is_empty")]
        metadata: Metadata,
    },
    /// Legacy binary resource reference. New inline bytes use `inline_binary`.
    Binary {
        /// Binary reference.
        binary: BinaryRef,
        /// Application metadata.
        #[serde(default, skip_serializing_if = "Metadata::is_empty")]
        metadata: Metadata,
    },
    /// Legacy product mode or planning hint.
    Mode {
        /// Mode name.
        mode: String,
        /// Optional structured configuration.
        #[serde(default, skip_serializing_if = "Value::is_null")]
        config: Value,
        /// Application metadata.
        #[serde(default, skip_serializing_if = "Metadata::is_empty")]
        metadata: Metadata,
    },
    /// Legacy slash-command or product command evidence.
    Command {
        /// Command name.
        command: String,
        /// Command arguments.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        args: Vec<String>,
        /// Optional structured payload.
        #[serde(default, skip_serializing_if = "Value::is_null")]
        payload: Value,
        /// Application metadata.
        #[serde(default, skip_serializing_if = "Metadata::is_empty")]
        metadata: Metadata,
    },
}

impl InputPart {
    /// Build text input.
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text {
            text: text.into(),
            metadata: Metadata::default(),
        }
    }

    /// Build an explicit image URL input.
    #[must_use]
    pub fn image_url(url: impl Into<String>) -> Self {
        Self::ImageUrl {
            url: url.into(),
            metadata: Metadata::default(),
        }
    }

    /// Build a legacy generic URL input.
    #[must_use]
    pub fn url(url: impl Into<String>) -> Self {
        Self::Url {
            url: url.into(),
            metadata: Metadata::default(),
        }
    }

    /// Build legacy command evidence.
    #[must_use]
    pub fn command(command: impl Into<String>, args: Vec<String>) -> Self {
        Self::Command {
            command: command.into(),
            args,
            payload: Value::Null,
            metadata: Metadata::default(),
        }
    }

    /// Return whether this part is a previous-release product-edge input.
    #[must_use]
    pub const fn is_legacy_product_input(&self) -> bool {
        matches!(self, Self::Mode { .. } | Self::Command { .. })
    }
}

impl From<ContentPart> for InputPart {
    fn from(part: ContentPart) -> Self {
        let metadata = Metadata::default();
        match part {
            ContentPart::CachePoint { ttl } => Self::CachePoint { ttl, metadata },
            ContentPart::Text { text } => Self::Text { text, metadata },
            ContentPart::ImageUrl { url } => Self::ImageUrl { url, metadata },
            ContentPart::FileUrl { url, media_type } => Self::FileUrl {
                url,
                media_type,
                metadata,
            },
            ContentPart::Binary { data, media_type } => Self::InlineBinary {
                data,
                media_type,
                metadata,
            },
            ContentPart::ResourceRef {
                uri,
                media_type,
                resource_type,
                metadata: resource_metadata,
            } => Self::ResourceRef {
                uri,
                media_type,
                resource_type,
                resource_metadata,
                metadata,
            },
            ContentPart::DataUrl {
                data_url,
                media_type,
            } => Self::DataUrl {
                data_url,
                media_type,
                metadata,
            },
        }
    }
}

impl TryFrom<InputPart> for ContentPart {
    type Error = InputConversionError;

    fn try_from(part: InputPart) -> Result<Self, Self::Error> {
        match part {
            InputPart::CachePoint { ttl, .. } => Ok(Self::CachePoint { ttl }),
            InputPart::Text { text, .. } => Ok(Self::Text { text }),
            InputPart::ImageUrl { url, .. } | InputPart::Url { url, .. } => {
                Ok(Self::ImageUrl { url })
            }
            InputPart::FileUrl {
                url, media_type, ..
            } => Ok(Self::FileUrl { url, media_type }),
            InputPart::InlineBinary {
                data, media_type, ..
            } => Ok(Self::Binary { data, media_type }),
            InputPart::ResourceRef {
                uri,
                media_type,
                resource_type,
                resource_metadata,
                ..
            } => Ok(Self::ResourceRef {
                uri,
                media_type,
                resource_type,
                metadata: resource_metadata,
            }),
            InputPart::DataUrl {
                data_url,
                media_type,
                ..
            } => Ok(Self::DataUrl {
                data_url,
                media_type,
            }),
            InputPart::File { file, .. } => {
                let mut metadata = Metadata::default();
                if let Some(name) = file.name {
                    metadata.insert("name".to_string(), Value::String(name));
                }
                Ok(Self::ResourceRef {
                    uri: file.uri,
                    media_type: file
                        .media_type
                        .unwrap_or_else(|| "application/octet-stream".to_string()),
                    resource_type: "file".to_string(),
                    metadata,
                })
            }
            InputPart::Binary { binary, .. } => {
                let mut metadata = Metadata::default();
                if let Some(bytes) = binary.bytes {
                    metadata.insert("bytes".to_string(), Value::from(bytes));
                }
                Ok(Self::ResourceRef {
                    uri: binary.uri,
                    media_type: binary
                        .media_type
                        .unwrap_or_else(|| "application/octet-stream".to_string()),
                    resource_type: "binary".to_string(),
                    metadata,
                })
            }
            InputPart::Mode { mode, .. } => Err(InputConversionError::ProductMode(mode)),
            InputPart::Command { command, .. } => {
                Err(InputConversionError::ProductCommand(command))
            }
        }
    }
}

/// Failure converting legacy durable input into canonical model content.
#[derive(Debug, Error)]
pub enum InputConversionError {
    /// Previous content-part escape hatch contains invalid content JSON.
    #[error("legacy content_part input is invalid: {0}")]
    LegacyContent(serde_json::Error),
    /// Product mode must be handled before runtime input conversion.
    #[error("product mode {0} is not model content")]
    ProductMode(String),
    /// Product command must be handled before runtime input conversion.
    #[error("product command {0} is not model content")]
    ProductCommand(String),
}
