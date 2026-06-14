//! Canonical tool call and tool return parts.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::Metadata;

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
