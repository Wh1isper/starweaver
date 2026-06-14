use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Metadata, RunId, TaskId};

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
