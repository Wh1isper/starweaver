//! Durable input part records.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use starweaver_core::Metadata;

/// Provider-scoped file reference submitted as session input.
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

/// Binary resource reference submitted as session input.
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

/// Serializable input submitted by CLI, API, bridges, schedules, and tools.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InputPart {
    /// Natural language prompt text.
    Text {
        /// Text content.
        text: String,
        /// Application metadata.
        #[serde(default, skip_serializing_if = "Metadata::is_empty")]
        metadata: Metadata,
    },
    /// URL reference.
    Url {
        /// URL string.
        url: String,
        /// Application metadata.
        #[serde(default, skip_serializing_if = "Metadata::is_empty")]
        metadata: Metadata,
    },
    /// Provider-scoped file reference.
    File {
        /// File reference.
        file: FileRef,
        /// Application metadata.
        #[serde(default, skip_serializing_if = "Metadata::is_empty")]
        metadata: Metadata,
    },
    /// Binary resource or upload reference.
    Binary {
        /// Binary reference.
        binary: BinaryRef,
        /// Application metadata.
        #[serde(default, skip_serializing_if = "Metadata::is_empty")]
        metadata: Metadata,
    },
    /// Execution mode or planning hint.
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
    /// Slash-command or product command input.
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

    /// Build URL input.
    #[must_use]
    pub fn url(url: impl Into<String>) -> Self {
        Self::Url {
            url: url.into(),
            metadata: Metadata::default(),
        }
    }

    /// Build command input.
    #[must_use]
    pub fn command(command: impl Into<String>, args: Vec<String>) -> Self {
        Self::Command {
            command: command.into(),
            args,
            payload: Value::Null,
            metadata: Metadata::default(),
        }
    }
}
