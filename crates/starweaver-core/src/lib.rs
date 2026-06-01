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

/// Session identifier shared by SDK, CLI, and service layers.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct SessionId(String);

impl SessionId {
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

/// Run identifier.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
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
        assert_eq!(AgentId::from_string("agent-1").as_str(), "agent-1");
        assert_eq!(
            SessionId::from_string("session-fixed").as_str(),
            "session-fixed"
        );
        assert!(RunId::new().as_str().starts_with("run_"));
        assert_eq!(RunId::from_string("run-fixed").as_str(), "run-fixed");
        assert!(ConversationId::new().as_str().starts_with("conv_"));
        assert_eq!(
            ConversationId::from_string("conv-fixed").as_str(),
            "conv-fixed"
        );
        assert!(CheckpointId::new().as_str().starts_with("ckpt_"));
        assert_eq!(
            CheckpointId::from_string("ckpt-fixed").as_str(),
            "ckpt-fixed"
        );
        assert!(TaskId::new().as_str().starts_with("task_"));
        assert_eq!(TaskId::from_string("task-fixed").as_str(), "task-fixed");
    }

    #[test]
    fn parses_and_builds_trace_context() {
        let context = TraceContext::from_trace_parent(
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
        )
        .with_span_id("span-1")
        .with_trace_state("vendor=state");

        assert_eq!(
            context.trace_id.as_deref(),
            Some("4bf92f3577b34da6a3ce929d0e0e4736")
        );
        assert_eq!(context.parent_span_id.as_deref(), Some("00f067aa0ba902b7"));
        assert_eq!(context.span_id.as_deref(), Some("span-1"));
        assert_eq!(context.trace_state.as_deref(), Some("vendor=state"));
        assert_eq!(context.metadata["trace_flags"], "01");
        assert!(!context.is_empty());

        let mut metadata = Metadata::default();
        metadata.insert("tenant".to_string(), Value::String("acme".to_string()));
        let fallback = TraceContext::from_trace_parent("trace-id")
            .with_parent_span_id("parent")
            .with_metadata(metadata.clone());
        assert_eq!(fallback.trace_id.as_deref(), Some("trace-id"));
        assert_eq!(fallback.parent_span_id.as_deref(), Some("parent"));
        assert_eq!(fallback.metadata, metadata);
        assert!(TraceContext::new().is_empty());
    }

    #[test]
    fn builds_subagent_specs_lifecycle_events_and_usage() {
        let spec = SubagentSpec::new("research", "Research helper", "Find facts")
            .with_tools(vec!["search".to_string()])
            .with_optional_tools(vec!["browser".to_string()]);
        assert_eq!(spec.name, "research");
        assert_eq!(spec.description, "Research helper");
        assert_eq!(spec.system_prompt, "Find facts");
        assert_eq!(spec.tools, ["search"]);
        assert_eq!(spec.optional_tools, ["browser"]);

        let event = SubagentLifecycleEvent::new(
            SubagentLifecycleKind::Completed,
            "research",
            TaskId::from_string("task-1"),
        )
        .with_run_id(RunId::from_string("run-1"))
        .with_metadata(serde_json::json!({"ok": true}));
        assert_eq!(event.kind, SubagentLifecycleKind::Completed);
        assert_eq!(event.name, "research");
        assert_eq!(event.task_id.as_str(), "task-1");
        assert_eq!(event.run_id.as_ref().map(RunId::as_str), Some("run-1"));
        assert_eq!(event.metadata["ok"], true);

        let mut usage = Usage {
            requests: 1,
            input_tokens: 2,
            output_tokens: 3,
            total_tokens: 5,
            tool_calls: 1,
        };
        usage.add_assign(&Usage {
            requests: 2,
            input_tokens: 4,
            output_tokens: 6,
            total_tokens: 10,
            tool_calls: 3,
        });
        assert_eq!(usage.requests, 3);
        assert_eq!(usage.input_tokens, 6);
        assert_eq!(usage.output_tokens, 9);
        assert_eq!(usage.total_tokens, 15);
        assert_eq!(usage.tool_calls, 4);
        assert_eq!(usage.with_additional_tool_calls(2).tool_calls, 6);
    }
}
