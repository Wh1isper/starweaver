//! Durable agent-owned state used by tool bundles.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use starweaver_core::Metadata;

use crate::TaskManager;

/// Durable agent-owned state used by tool bundles.
///
/// This component keeps shell, task, deferred-call, handoff-file, and dynamic
/// tool-search state out of the lifecycle-wide [`crate::AgentContext`] root.
/// It is flattened during context serialization to preserve the established
/// v0 wire shape while giving runtime code an explicit ownership boundary.
#[derive(Clone, Default, Deserialize, Serialize)]
pub struct AgentToolState {
    /// Legacy execution environment accepted on restore but never serialized.
    #[serde(default, rename = "shell_env", skip_serializing)]
    pub shell_environment: BTreeMap<String, String>,
    /// Metadata for deferred tool calls.
    #[serde(
        default,
        rename = "deferred_tool_metadata",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub deferred_call_metadata: BTreeMap<String, Metadata>,
    /// Files to auto-load on the next request.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub auto_load_files: Vec<String>,
    /// Agent-managed task state.
    #[serde(
        default,
        rename = "task_manager",
        skip_serializing_if = "TaskManager::is_empty"
    )]
    pub tasks: TaskManager,
    /// Tool names loaded through dynamic tool search.
    #[serde(
        default,
        rename = "tool_search_loaded_tools",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub loaded_tool_names: Vec<String>,
    /// Namespace identifiers loaded through dynamic tool search.
    #[serde(
        default,
        rename = "tool_search_loaded_namespaces",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub loaded_tool_namespaces: Vec<String>,
}

impl std::fmt::Debug for AgentToolState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AgentToolState")
            .field(
                "shell_environment_keys",
                &self.shell_environment.keys().collect::<Vec<_>>(),
            )
            .field("deferred_call_metadata", &self.deferred_call_metadata)
            .field("auto_load_files", &self.auto_load_files)
            .field("tasks", &self.tasks)
            .field("loaded_tool_names", &self.loaded_tool_names)
            .field("loaded_tool_namespaces", &self.loaded_tool_namespaces)
            .finish()
    }
}
