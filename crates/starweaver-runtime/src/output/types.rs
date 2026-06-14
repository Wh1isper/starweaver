use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{OutputValidationError, OutputValidationResult};

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

/// Parsed final output value.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum OutputValue {
    /// Plain text output.
    Text(String),
    /// Structured JSON output.
    Json(Value),
}

impl OutputValue {
    /// Return text output, serializing JSON when needed.
    #[must_use]
    pub fn as_text(&self) -> String {
        match self {
            Self::Text(text) => text.clone(),
            Self::Json(value) => value.to_string(),
        }
    }

    /// Return JSON output when this value is structured.
    #[must_use]
    pub const fn as_json(&self) -> Option<&Value> {
        match self {
            Self::Text(_) => None,
            Self::Json(value) => Some(value),
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
        };
        serde_json::from_value(value)
            .map_err(|error| OutputValidationError::Schema(error.to_string()))
    }
}
