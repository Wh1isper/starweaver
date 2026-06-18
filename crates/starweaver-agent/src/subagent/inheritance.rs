use serde::{Deserialize, Serialize};
use starweaver_runtime::{AgentCapability, CapabilityBundle};
use starweaver_tools::ToolRegistry;

/// Tool inheritance policy for SDK-level subagent delegation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SubagentToolInheritancePolicy {
    /// Inherit tools whose metadata includes `auto_inherit=true`.
    #[serde(default = "default_auto_inherit")]
    pub auto_inherit: bool,
    /// Inherit every parent tool except denied and nested delegation tools.
    #[serde(default, skip_serializing_if = "is_false")]
    pub inherit_all_when_empty: bool,
    /// Required parent tool names.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_tools: Vec<String>,
    /// Optional parent tool names.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub optional_tools: Vec<String>,
    /// Parent tool names withheld from the child registry.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub denied_tools: Vec<String>,
    /// Whether nested delegation tools can be inherited.
    #[serde(default, skip_serializing_if = "is_false")]
    pub allow_nested_delegation: bool,
}

impl Default for SubagentToolInheritancePolicy {
    fn default() -> Self {
        Self {
            auto_inherit: true,
            inherit_all_when_empty: false,
            required_tools: Vec::new(),
            optional_tools: Vec::new(),
            denied_tools: Vec::new(),
            allow_nested_delegation: false,
        }
    }
}

impl SubagentToolInheritancePolicy {
    /// Build a policy from required and optional inherited tool lists.
    #[must_use]
    pub fn new(required_tools: Vec<String>, optional_tools: Vec<String>) -> Self {
        Self {
            required_tools,
            optional_tools,
            ..Self::default()
        }
    }

    /// Inherit all parent tools when no explicit required or optional tools are configured.
    #[must_use]
    pub const fn with_inherit_all_when_empty(mut self, enabled: bool) -> Self {
        self.inherit_all_when_empty = enabled;
        self
    }

    /// Disable metadata-driven auto inheritance.
    #[must_use]
    pub const fn without_auto_inherit(mut self) -> Self {
        self.auto_inherit = false;
        self
    }

    /// Add denied tool names.
    #[must_use]
    pub fn with_denied_tools(mut self, denied_tools: Vec<String>) -> Self {
        self.denied_tools = denied_tools;
        self
    }

    /// Allow inheriting delegation tools for nested coordination.
    #[must_use]
    pub const fn with_nested_delegation(mut self, allowed: bool) -> Self {
        self.allow_nested_delegation = allowed;
        self
    }

    /// Resolve inherited tools from a parent registry.
    ///
    /// # Errors
    ///
    /// Returns an error when required tools are missing or denied.
    pub fn resolve(
        &self,
        parent: &ToolRegistry,
    ) -> Result<ToolRegistry, SubagentToolInheritanceError> {
        let explicit_tools = !self.required_tools.is_empty() || !self.optional_tools.is_empty();
        let mut inherited = if self.inherit_all_when_empty && !explicit_tools {
            parent.clone()
        } else if self.auto_inherit {
            parent.auto_inherited()
        } else {
            ToolRegistry::new()
        };
        for name in &self.optional_tools {
            if let Some(tool) = parent.get(name) {
                inherited.insert(tool);
            }
        }
        for name in &self.required_tools {
            if self.denied_tools.contains(name) {
                return Err(SubagentToolInheritanceError::DeniedRequiredTool(
                    name.clone(),
                ));
            }
            let tool = parent
                .get(name)
                .ok_or_else(|| SubagentToolInheritanceError::MissingRequiredTool(name.clone()))?;
            inherited.insert(tool);
        }
        for name in &self.denied_tools {
            inherited.remove(name);
        }
        if !self.allow_nested_delegation {
            inherited.remove("delegate");
            inherited.remove("subagent_info");
        }
        Ok(inherited)
    }
}

/// Subagent inherited-tool resolution failure.
#[derive(Debug, thiserror::Error)]
pub enum SubagentToolInheritanceError {
    /// Required tool was not present in the parent registry.
    #[error("required inherited tool is missing: {0}")]
    MissingRequiredTool(String),
    /// Required tool was also listed as denied.
    #[error("required inherited tool is denied: {0}")]
    DeniedRequiredTool(String),
}

/// Capability inheritance policy for SDK-level subagent delegation.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SubagentCapabilityInheritancePolicy {
    /// Inherit parent runtime hook capabilities registered through the SDK builder.
    #[serde(default, skip_serializing_if = "is_false")]
    pub hooks: bool,
    /// Inherit parent capability bundles registered through the SDK builder.
    #[serde(default, skip_serializing_if = "is_false")]
    pub capability_bundles: bool,
    /// Capability ids or bundle names withheld from the child agent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub denied_capabilities: Vec<String>,
}

impl SubagentCapabilityInheritancePolicy {
    /// Inherit parent runtime hook capabilities.
    #[must_use]
    pub const fn with_hooks(mut self, enabled: bool) -> Self {
        self.hooks = enabled;
        self
    }

    /// Inherit parent capability bundles.
    #[must_use]
    pub const fn with_capability_bundles(mut self, enabled: bool) -> Self {
        self.capability_bundles = enabled;
        self
    }

    /// Add denied capability ids or bundle names.
    #[must_use]
    pub fn with_denied_capabilities(mut self, denied_capabilities: Vec<String>) -> Self {
        self.denied_capabilities = denied_capabilities;
        self
    }

    pub(crate) fn allows_hook(&self, capability: &std::sync::Arc<dyn AgentCapability>) -> bool {
        self.allows(capability.spec().id.as_str())
    }

    pub(crate) fn allows_bundle(&self, bundle: &std::sync::Arc<dyn CapabilityBundle>) -> bool {
        self.allows(bundle.spec().id.as_str()) && self.allows(bundle.name())
    }

    fn allows(&self, id: &str) -> bool {
        !self.denied_capabilities.iter().any(|denied| denied == id)
    }
}

const fn default_auto_inherit() -> bool {
    true
}

#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_false(value: &bool) -> bool {
    !*value
}
