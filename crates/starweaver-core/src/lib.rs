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

/// Trace context shared by SDK, runtime, model, service, and observability layers.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct TraceContext {
    /// Trace identifier from an external root trace or local tracer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    /// Current span identifier when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span_id: Option<String>,
    /// Parent span identifier when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_span_id: Option<String>,
    /// W3C trace state or collector-specific propagation state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_state: Option<String>,
    /// Additional low-cardinality trace metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

impl TraceContext {
    /// Create an empty trace context.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a trace context from an external trace id.
    #[must_use]
    pub fn from_trace_id(trace_id: impl Into<String>) -> Self {
        Self {
            trace_id: Some(trace_id.into()),
            ..Self::default()
        }
    }

    /// Create a trace context from a W3C traceparent header.
    #[must_use]
    pub fn from_trace_parent(trace_parent: impl Into<String>) -> Self {
        let trace_parent = trace_parent.into();
        let parts = trace_parent.split('-').collect::<Vec<_>>();
        if parts.len() >= 4 {
            let mut metadata = Metadata::default();
            metadata.insert(
                "trace_flags".to_string(),
                Value::String(parts[3].to_string()),
            );
            Self {
                trace_id: Some(parts[1].to_string()),
                parent_span_id: Some(parts[2].to_string()),
                metadata,
                ..Self::default()
            }
        } else {
            Self::from_trace_id(trace_parent)
        }
    }

    /// Attach a span id.
    #[must_use]
    pub fn with_span_id(mut self, span_id: impl Into<String>) -> Self {
        self.span_id = Some(span_id.into());
        self
    }

    /// Attach a parent span id.
    #[must_use]
    pub fn with_parent_span_id(mut self, parent_span_id: impl Into<String>) -> Self {
        self.parent_span_id = Some(parent_span_id.into());
        self
    }

    /// Attach trace state.
    #[must_use]
    pub fn with_trace_state(mut self, trace_state: impl Into<String>) -> Self {
        self.trace_state = Some(trace_state.into());
        self
    }

    /// Attach metadata.
    #[must_use]
    pub fn with_metadata(mut self, metadata: Metadata) -> Self {
        self.metadata = metadata;
        self
    }

    /// Return whether the trace context is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.trace_id.is_none()
            && self.span_id.is_none()
            && self.parent_span_id.is_none()
            && self.trace_state.is_none()
            && self.metadata.is_empty()
    }
}

/// Checkpoint identifier shared by runtime and service layers.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct CheckpointId(String);

impl CheckpointId {
    /// Create a new random checkpoint identifier.
    #[must_use]
    pub fn new() -> Self {
        Self(format!("ckpt_{}", Uuid::new_v4()))
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

impl Default for CheckpointId {
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
        assert!(CheckpointId::new().as_str().starts_with("ckpt_"));
        assert!(TaskId::new().as_str().starts_with("task_"));
    }

    #[test]
    fn parses_w3c_trace_parent() {
        let context = TraceContext::from_trace_parent(
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
        );

        assert_eq!(
            context.trace_id.as_deref(),
            Some("4bf92f3577b34da6a3ce929d0e0e4736")
        );
        assert_eq!(context.parent_span_id.as_deref(), Some("00f067aa0ba902b7"));
        assert_eq!(context.metadata["trace_flags"], "01");
    }
}
