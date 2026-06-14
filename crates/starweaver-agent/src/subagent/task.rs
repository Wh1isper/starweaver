use starweaver_core::TaskId;
use starweaver_runtime::AgentResult;

/// Application-level task envelope used for SDK subagent delegation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubagentTask {
    /// Stable task identifier shared across runtime, service, and SDK layers.
    pub id: TaskId,
    /// Prompt delegated to the subagent.
    pub prompt: String,
    /// Application metadata carried with the delegated task.
    pub metadata: serde_json::Value,
}

impl SubagentTask {
    /// Build a subagent task from a prompt.
    #[must_use]
    pub fn new(prompt: impl Into<String>) -> Self {
        Self {
            id: TaskId::new(),
            prompt: prompt.into(),
            metadata: serde_json::Value::Object(serde_json::Map::new()),
        }
    }

    /// Set a caller-provided task identifier.
    #[must_use]
    pub fn with_id(mut self, id: TaskId) -> Self {
        self.id = id;
        self
    }

    /// Attach application metadata to the delegated task.
    #[must_use]
    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = metadata;
        self
    }
}

/// Application-level result envelope returned by SDK subagent delegation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubagentResult {
    /// Subagent name used for delegation.
    pub name: String,
    /// Delegated task envelope.
    pub task: SubagentTask,
    /// Runtime result produced by the delegated subagent.
    pub result: AgentResult,
}

impl SubagentResult {
    /// Return the final text output produced by the subagent.
    #[must_use]
    pub fn output(&self) -> &str {
        &self.result.output
    }

    /// Consume the envelope and return the underlying runtime result.
    #[must_use]
    pub fn into_result(self) -> AgentResult {
        self.result
    }
}
