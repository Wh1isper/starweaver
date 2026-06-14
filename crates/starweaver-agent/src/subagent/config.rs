use std::sync::Arc;

use starweaver_runtime::Agent as RuntimeAgent;

use super::SubagentToolInheritancePolicy;

/// Registered subagent configuration for SDK-level delegation.
#[derive(Clone)]
pub struct SubagentConfig {
    /// Subagent name exposed to application delegation policies.
    pub name: String,
    /// Optional subagent description.
    pub description: Option<String>,
    /// Nested agent runtime.
    pub agent: Arc<RuntimeAgent>,
    /// Tool inheritance policy applied before every delegated run.
    pub tool_inheritance: SubagentToolInheritancePolicy,
}

impl SubagentConfig {
    /// Build a subagent configuration.
    #[must_use]
    pub fn new(name: impl Into<String>, agent: Arc<RuntimeAgent>) -> Self {
        Self {
            name: name.into(),
            description: None,
            agent,
            tool_inheritance: SubagentToolInheritancePolicy::default(),
        }
    }

    /// Add a description.
    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set the tool inheritance policy used for this subagent.
    #[must_use]
    pub fn with_tool_inheritance(mut self, policy: SubagentToolInheritancePolicy) -> Self {
        self.tool_inheritance = policy;
        self
    }
}
