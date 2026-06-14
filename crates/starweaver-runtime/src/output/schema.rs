use serde::{Deserialize, Serialize};
use serde_json::Value;

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
