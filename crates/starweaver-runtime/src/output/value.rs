use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{OutputValidationError, OutputValidationResult};

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
