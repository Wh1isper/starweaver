//! Core abstractions for the Starweaver agent SDK.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use uuid::Uuid;

/// Workspace-wide SDK identity.
pub const SDK_NAME: &str = "starweaver-agent-sdk";

/// Serializable metadata object shared by Starweaver crates.
pub type Metadata = Map<String, Value>;

/// Run identifier.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct RunId(String);

impl RunId {
    /// Create a new random run identifier.
    #[must_use]
    pub fn new() -> Self {
        Self(format!("run_{}", Uuid::new_v4()))
    }

    /// Create an identifier from a caller-provided string.
    #[must_use]
    pub fn from_string(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Return the string representation.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for RunId {
    fn default() -> Self {
        Self::new()
    }
}

/// Conversation identifier.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct ConversationId(String);

impl ConversationId {
    /// Create a new random conversation identifier.
    #[must_use]
    pub fn new() -> Self {
        Self(format!("conv_{}", Uuid::new_v4()))
    }

    /// Create an identifier from a caller-provided string.
    #[must_use]
    pub fn from_string(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Return the string representation.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for ConversationId {
    fn default() -> Self {
        Self::new()
    }
}

/// Token and request usage accumulated by model and runtime layers.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Usage {
    /// Number of provider requests.
    pub requests: u64,
    /// Input or prompt tokens.
    pub input_tokens: u64,
    /// Output or completion tokens.
    pub output_tokens: u64,
    /// Total tokens.
    pub total_tokens: u64,
    /// Number of successful function tool calls executed by the runtime.
    #[serde(default)]
    pub tool_calls: u64,
}

impl Usage {
    /// Add another usage value into this one.
    pub fn add_assign(&mut self, other: &Self) {
        self.requests += other.requests;
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.total_tokens += other.total_tokens;
        self.tool_calls += other.tool_calls;
    }

    /// Return a copy with additional successful tool calls applied.
    #[must_use]
    pub const fn with_additional_tool_calls(mut self, tool_calls: u64) -> Self {
        self.tool_calls = self.tool_calls.saturating_add(tool_calls);
        self
    }
}

/// Returns the SDK name used across commands and diagnostics.
#[must_use]
pub const fn sdk_name() -> &'static str {
    SDK_NAME
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_sdk_name() {
        assert_eq!(sdk_name(), "starweaver-agent-sdk");
    }

    #[test]
    fn creates_prefixed_ids() {
        assert!(RunId::new().as_str().starts_with("run_"));
        assert!(ConversationId::new().as_str().starts_with("conv_"));
    }
}
