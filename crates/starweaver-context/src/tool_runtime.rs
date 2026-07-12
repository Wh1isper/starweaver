//! Read-only context projection supplied to tool calls.

use std::collections::BTreeMap;

use crate::{ModelConfig, ToolConfig};

/// Narrow, read-only configuration snapshot available during tool execution.
///
/// The snapshot deliberately excludes mutable conversation, task, note, message,
/// event, usage, and lifecycle state. Tools that need coordinated mutation use
/// dedicated handles such as [`crate::AgentContextHandle`] until narrower
/// capability-specific handles replace them.
#[derive(Clone, Default, Eq, PartialEq)]
pub struct ToolRuntimeSnapshot {
    model_config: ModelConfig,
    tool_config: ToolConfig,
    shell_environment: BTreeMap<String, String>,
}

impl std::fmt::Debug for ToolRuntimeSnapshot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ToolRuntimeSnapshot")
            .field("model_config", &self.model_config)
            .field("tool_config", &self.tool_config)
            .field(
                "shell_environment_keys",
                &self.shell_environment.keys().collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl ToolRuntimeSnapshot {
    pub(crate) const fn new(
        model_config: ModelConfig,
        tool_config: ToolConfig,
        shell_environment: BTreeMap<String, String>,
    ) -> Self {
        Self {
            model_config,
            tool_config,
            shell_environment,
        }
    }

    pub(crate) const fn filtered(model_config: ModelConfig, tool_config: ToolConfig) -> Self {
        Self {
            model_config,
            tool_config,
            shell_environment: BTreeMap::new(),
        }
    }

    /// Return the model limits relevant to media-producing tools.
    #[must_use]
    pub const fn model_config(&self) -> &ModelConfig {
        &self.model_config
    }

    /// Return the SDK tool policy and resource limits.
    #[must_use]
    pub const fn tool_config(&self) -> &ToolConfig {
        &self.tool_config
    }

    /// Return the base environment variables configured for shell tools.
    #[must_use]
    pub const fn shell_environment(&self) -> &BTreeMap<String, String> {
        &self.shell_environment
    }
}

/// Dedicated configured shell environment supplied only to authorized tool calls.
#[derive(Clone, Default, Eq, PartialEq)]
pub struct ShellEnvironmentSnapshot {
    environment: BTreeMap<String, String>,
}

impl ShellEnvironmentSnapshot {
    pub(crate) const fn new(environment: BTreeMap<String, String>) -> Self {
        Self { environment }
    }

    /// Return configured shell environment values.
    #[must_use]
    pub const fn environment(&self) -> &BTreeMap<String, String> {
        &self.environment
    }
}

impl std::fmt::Debug for ShellEnvironmentSnapshot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ShellEnvironmentSnapshot")
            .field(
                "environment_keys",
                &self.environment.keys().collect::<Vec<_>>(),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::ShellEnvironmentSnapshot;

    #[test]
    fn shell_environment_snapshot_debug_omits_values() {
        let snapshot = ShellEnvironmentSnapshot::new(BTreeMap::from([(
            "STARWEAVER_SECRET".to_string(),
            "STARWEAVER_SECRET_SENTINEL_7f9c".to_string(),
        )]));

        let debug = format!("{snapshot:?}");
        assert!(debug.contains("STARWEAVER_SECRET"));
        assert!(!debug.contains("STARWEAVER_SECRET_SENTINEL_7f9c"));
    }
}
