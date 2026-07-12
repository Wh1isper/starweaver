//! Ephemeral state used only while an agent context is executing.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use starweaver_core::Metadata;

use crate::{
    AgentStreamQueueRegistry, ContextLifecycleState, ToolCapabilityGrant, ToolIdWrapper,
    WrapperMetadata,
};

/// Runtime-only context state excluded from resumable session snapshots.
///
/// This component is flattened into [`crate::AgentContext`] serialization to preserve the
/// pre-decomposition JSON shape. Durable restoration continues to use
/// [`crate::ResumableState`], which deliberately does not include these fields.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RuntimeEphemeralState {
    /// Force environment/runtime context injection on the next filter pass.
    #[serde(default)]
    pub force_inject_context: bool,
    /// Context-injection tag names that should be stripped or refreshed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub injected_context_tags: Vec<String>,
    /// Active context management tool names.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context_manage_tool_names: Vec<String>,
    /// Active tool capability tags.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_tags: Vec<String>,
    /// Tool call ID wrapper for provider-normalized tool IDs.
    #[serde(default, skip_serializing_if = "ToolIdWrapper::is_empty")]
    pub tool_id_wrapper: ToolIdWrapper,
    /// Runtime stream queue registry placeholder.
    #[serde(default, skip_serializing_if = "AgentStreamQueueRegistry::is_empty")]
    pub agent_stream_queues: AgentStreamQueueRegistry,
    /// Wrapper metadata carried by the running context.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub wrapper_metadata: WrapperMetadata,
    /// Runtime lifecycle state.
    #[serde(default, skip_serializing_if = "ContextLifecycleState::is_default")]
    pub lifecycle: ContextLifecycleState,
    /// Whether run-scoped toolsets have completed their exit lifecycle.
    #[serde(skip, default = "default_run_toolsets_closed")]
    pub run_toolsets_closed: bool,
    /// Current runtime run step for context-aware preparation.
    #[serde(skip)]
    pub current_run_step: usize,
    /// Host-authorized per-tool grants, never persisted in resumable state.
    #[serde(skip)]
    pub tool_capability_grants: BTreeMap<String, ToolCapabilityGrant>,
}

impl Default for RuntimeEphemeralState {
    fn default() -> Self {
        Self {
            force_inject_context: false,
            injected_context_tags: default_injected_context_tags(),
            context_manage_tool_names: Vec::new(),
            tool_tags: Vec::new(),
            tool_id_wrapper: ToolIdWrapper::default(),
            agent_stream_queues: AgentStreamQueueRegistry::default(),
            wrapper_metadata: Metadata::default(),
            lifecycle: ContextLifecycleState::default(),
            run_toolsets_closed: true,
            current_run_step: 0,
            tool_capability_grants: BTreeMap::new(),
        }
    }
}

const fn default_run_toolsets_closed() -> bool {
    true
}

fn default_injected_context_tags() -> Vec<String> {
    vec![
        "runtime-context".to_string(),
        "environment-context".to_string(),
    ]
}
