use serde::{Deserialize, Serialize};
use serde_json::Value;
use starweaver_model::ModelResponsePart;

use super::{OutputValidationError, OutputValidationResult};

fn typed_schema_name<T>() -> String {
    let raw = std::any::type_name::<T>()
        .rsplit("::")
        .next()
        .unwrap_or("output");
    let mut name = String::new();
    let mut previous_was_separator = false;
    for character in raw.chars() {
        if character.is_ascii_alphanumeric() {
            name.push(character.to_ascii_lowercase());
            previous_was_separator = false;
        } else if !previous_was_separator && !name.is_empty() {
            name.push('_');
            previous_was_separator = true;
        }
    }
    while name.ends_with('_') {
        name.pop();
    }
    if name.is_empty() {
        "output".to_string()
    } else {
        name
    }
}

/// Structured output schema passed to the model and used for runtime parsing.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OutputSchema {
    /// Output schema name.
    pub name: String,
    /// Output schema description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON schema for the final output value.
    pub schema: Value,
    /// Whether schema validation should be strict when provider support exists.
    #[serde(default)]
    pub strict: bool,
}

impl OutputSchema {
    /// Build a named output schema.
    #[must_use]
    pub fn new(name: impl Into<String>, schema: Value) -> Self {
        Self {
            name: name.into(),
            description: None,
            schema,
            strict: true,
        }
    }

    /// Build an output schema from a Rust type.
    ///
    /// The generated schema name is derived from the Rust type name and normalized for
    /// provider request formats. Use [`Self::typed_named`] when the public schema name
    /// needs to be stable across type renames.
    #[must_use]
    pub fn typed<T>() -> Self
    where
        T: schemars::JsonSchema,
    {
        Self::typed_named::<T>(typed_schema_name::<T>())
    }

    /// Build a named output schema from a Rust type.
    #[must_use]
    pub fn typed_named<T>(name: impl Into<String>) -> Self
    where
        T: schemars::JsonSchema,
    {
        let schema = schemars::schema_for!(T);
        let schema = serde_json::to_value(schema).unwrap_or_else(|_| {
            serde_json::json!({
                "type": "object",
                "properties": {},
            })
        });
        Self::new(name, schema)
    }

    /// Add a description.
    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set strict provider validation preference.
    #[must_use]
    pub const fn with_strict(mut self, strict: bool) -> Self {
        self.strict = strict;
        self
    }

    /// Return provider-neutral request schema metadata.
    #[must_use]
    pub fn request_schema(&self) -> Value {
        serde_json::json!({
            "name": self.name,
            "description": self.description,
            "schema": self.schema,
            "strict": self.strict,
        })
    }
}

/// Media/file output returned by the model.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OutputMedia {
    /// Output URL or provider resource URI.
    pub url: String,
    /// Media type reported by the provider.
    pub media_type: String,
}

impl OutputMedia {
    /// Build an output media wrapper.
    #[must_use]
    pub fn new(url: impl Into<String>, media_type: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            media_type: media_type.into(),
        }
    }

    /// Return true when this media output is an image.
    #[must_use]
    pub fn is_image(&self) -> bool {
        self.media_type.starts_with("image/")
    }

    /// Return true when this media output is a video.
    #[must_use]
    pub fn is_video(&self) -> bool {
        self.media_type.starts_with("video/")
    }

    /// Return true when this media output is audio.
    #[must_use]
    pub fn is_audio(&self) -> bool {
        self.media_type.starts_with("audio/")
    }

    /// Return true when this media output looks like a document or generic file.
    #[must_use]
    pub fn is_file(&self) -> bool {
        !self.is_image() && !self.is_video() && !self.is_audio()
    }

    /// Build an output media wrapper from a canonical model response part.
    #[must_use]
    pub fn from_response_part(part: &ModelResponsePart) -> Option<Self> {
        match part {
            ModelResponsePart::File { url, media_type } => Some(Self::new(url, media_type)),
            ModelResponsePart::Text { .. }
            | ModelResponsePart::ProviderText { .. }
            | ModelResponsePart::Thinking { .. }
            | ModelResponsePart::ProviderThinking { .. }
            | ModelResponsePart::ToolCall(_)
            | ModelResponsePart::ProviderToolCall { .. }
            | ModelResponsePart::NativeToolCall { .. }
            | ModelResponsePart::NativeToolReturn { .. }
            | ModelResponsePart::Compaction { .. }
            | ModelResponsePart::ProviderOpaque { .. } => None,
        }
    }
}

/// Parsed final output value.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum OutputValue {
    /// Plain text output.
    Text(String),
    /// Structured JSON output.
    Json(Value),
    /// Media/file outputs.
    Media(Vec<OutputMedia>),
}

impl OutputValue {
    /// Return text output, serializing JSON when needed.
    #[must_use]
    pub fn as_text(&self) -> String {
        match self {
            Self::Text(text) => text.clone(),
            Self::Json(value) => value.to_string(),
            Self::Media(media) => serde_json::to_string(media).unwrap_or_else(|_| "[]".to_string()),
        }
    }

    /// Return JSON output when this value is structured.
    #[must_use]
    pub const fn as_json(&self) -> Option<&Value> {
        match self {
            Self::Json(value) => Some(value),
            Self::Text(_) | Self::Media(_) => None,
        }
    }

    /// Parse this output value into a Rust type.
    ///
    /// # Errors
    ///
    /// Returns an error when text is not valid JSON or deserialization fails.
    pub fn parse<T>(&self) -> OutputValidationResult<T>
    where
        T: serde::de::DeserializeOwned,
    {
        let value = match self {
            Self::Text(text) => serde_json::from_str(text)
                .map_err(|error| OutputValidationError::InvalidJson(error.to_string()))?,
            Self::Json(value) => value.clone(),
            Self::Media(media) => serde_json::to_value(media)
                .map_err(|error| OutputValidationError::Schema(error.to_string()))?,
        };
        serde_json::from_value(value)
            .map_err(|error| OutputValidationError::Schema(error.to_string()))
    }
}
