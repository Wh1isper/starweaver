//! Core abstractions for the Starweaver agent SDK.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use uuid::Uuid;

/// Workspace-wide SDK identity.
pub const SDK_NAME: &str = "starweaver-agent-sdk";

/// Serializable metadata object shared by Starweaver crates.
pub type Metadata = Map<String, Value>;

/// Runtime agent identifier.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct AgentId(String);

impl AgentId {
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

impl Default for AgentId {
    fn default() -> Self {
        Self("main".to_string())
    }
}

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

/// Task identifier shared by runtime, SDK, and service layers.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct TaskId(String);

impl TaskId {
    /// Create a new random task identifier.
    #[must_use]
    pub fn new() -> Self {
        Self(format!("task_{}", Uuid::new_v4()))
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

impl Default for TaskId {
    fn default() -> Self {
        Self::new()
    }
}

/// Serializable subagent configuration shared by SDK, service, and CLI layers.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SubagentSpec {
    /// Unique subagent name.
    pub name: String,
    /// Description shown to delegation policies and model-facing tools.
    pub description: String,
    /// Optional short instruction for parent-facing delegation guidance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instruction: Option<String>,
    /// Subagent system prompt.
    pub system_prompt: String,
    /// Required inherited tool names.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<String>,
    /// Optional inherited tool names.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub optional_tools: Vec<String>,
    /// Optional model override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Optional model settings preset or structured configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_settings: Option<Value>,
    /// Optional model capability/profile preset or structured configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_config: Option<Value>,
    /// Application metadata for future service and CLI integration.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

impl SubagentSpec {
    /// Build a serializable subagent specification.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        system_prompt: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            system_prompt: system_prompt.into(),
            ..Self::default()
        }
    }

    /// Attach required inherited tool names.
    #[must_use]
    pub fn with_tools(mut self, tools: Vec<String>) -> Self {
        self.tools = tools;
        self
    }

    /// Attach optional inherited tool names.
    #[must_use]
    pub fn with_optional_tools(mut self, optional_tools: Vec<String>) -> Self {
        self.optional_tools = optional_tools;
        self
    }
}

/// Subagent lifecycle event kind shared by SDK, service, and CLI layers.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentLifecycleKind {
    /// Delegation started.
    Started,
    /// Delegation completed successfully.
    Completed,
    /// Delegation failed.
    Failed,
}

/// Serializable subagent lifecycle event payload.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SubagentLifecycleEvent {
    /// Lifecycle event kind.
    pub kind: SubagentLifecycleKind,
    /// Subagent name.
    pub name: String,
    /// Delegated task identifier.
    pub task_id: TaskId,
    /// Related run identifier when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    /// Application metadata carried with the task or error.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub metadata: Value,
}

impl SubagentLifecycleEvent {
    /// Build a lifecycle event payload.
    #[must_use]
    pub fn new(kind: SubagentLifecycleKind, name: impl Into<String>, task_id: TaskId) -> Self {
        Self {
            kind,
            name: name.into(),
            task_id,
            run_id: None,
            metadata: Value::Null,
        }
    }

    /// Attach run identifier.
    #[must_use]
    pub fn with_run_id(mut self, run_id: RunId) -> Self {
        self.run_id = Some(run_id);
        self
    }

    /// Attach metadata.
    #[must_use]
    pub fn with_metadata(mut self, metadata: Value) -> Self {
        self.metadata = metadata;
        self
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
        assert_eq!(AgentId::default().as_str(), "main");
        assert!(RunId::new().as_str().starts_with("run_"));
        assert!(ConversationId::new().as_str().starts_with("conv_"));
        assert!(TaskId::new().as_str().starts_with("task_"));
    }
}
