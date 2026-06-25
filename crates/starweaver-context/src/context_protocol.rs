//! Context protocol support types carried by [`crate::AgentContext`].

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use starweaver_core::Metadata;
use starweaver_model::{ModelMessage, ModelRequestPart, ModelResponsePart};
use uuid::Uuid;

/// Metadata for a registered agent.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentInfo {
    /// Unique identifier for the agent.
    pub agent_id: String,
    /// Human-readable agent name.
    pub agent_name: String,
    /// Parent agent id, if this is a subagent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_agent_id: Option<String>,
}

impl AgentInfo {
    /// Create agent metadata.
    #[must_use]
    pub fn new(agent_id: impl Into<String>, agent_name: impl Into<String>) -> Self {
        Self {
            agent_id: agent_id.into(),
            agent_name: agent_name.into(),
            parent_agent_id: None,
        }
    }

    /// Attach parent agent id.
    #[must_use]
    pub fn with_parent_agent_id(mut self, parent_agent_id: impl Into<String>) -> Self {
        self.parent_agent_id = Some(parent_agent_id.into());
        self
    }
}

/// Metadata for one deferred tool call.
pub type DeferredToolMetadata = Metadata;

/// Runtime wrapper metadata passed to model/subagent wrapper equivalents.
pub type WrapperMetadata = Metadata;

/// Context lifecycle fields for enter, exit, streaming, and compaction state.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ContextLifecycleState {
    /// Whether the context has been entered.
    #[serde(default)]
    pub entered: bool,
    /// Whether stream queue side-channel behavior is enabled.
    #[serde(default)]
    pub stream_queue_enabled: bool,
    /// Current compact recursion depth.
    #[serde(default)]
    pub compact_depth: u32,
}

impl ContextLifecycleState {
    /// Return whether this state has default values.
    #[must_use]
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

/// Tool call ID normalizer for cross-provider tool call matching.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolIdWrapper {
    /// Prefix for normalized tool call IDs.
    #[serde(default = "default_tool_id_prefix")]
    pub prefix: String,
    /// Original provider id to normalized id mapping.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tool_call_maps: BTreeMap<String, String>,
}

impl Default for ToolIdWrapper {
    fn default() -> Self {
        Self {
            prefix: default_tool_id_prefix(),
            tool_call_maps: BTreeMap::new(),
        }
    }
}

impl ToolIdWrapper {
    /// Clear cached mappings.
    pub fn clear(&mut self) {
        self.tool_call_maps.clear();
    }

    /// Normalize a tool call id.
    pub fn upsert_tool_call_id(&mut self, tool_call_id: &str) -> String {
        if tool_call_id.starts_with(&self.prefix) {
            return tool_call_id.to_string();
        }
        if let Some(existing) = self.tool_call_maps.get(tool_call_id) {
            return existing.clone();
        }
        let wrapped = format!("{}{}", self.prefix, Uuid::new_v4().simple());
        self.tool_call_maps
            .insert(tool_call_id.to_string(), wrapped.clone());
        wrapped
    }

    /// Wrap all tool call IDs in a message history in place.
    pub fn wrap_messages(&mut self, message_history: &mut [ModelMessage]) {
        for message in message_history {
            self.wrap_message(message);
        }
    }

    /// Wrap all tool call IDs in one message in place.
    pub fn wrap_message(&mut self, message: &mut ModelMessage) {
        match message {
            ModelMessage::Request(request) => {
                for part in &mut request.parts {
                    match part {
                        ModelRequestPart::ToolReturn(tool_return) => {
                            tool_return.tool_call_id =
                                self.upsert_tool_call_id(&tool_return.tool_call_id);
                        }
                        ModelRequestPart::RetryPrompt { tool_call_id, .. } => {
                            if let Some(id) = tool_call_id {
                                *id = self.upsert_tool_call_id(id);
                            }
                        }
                        ModelRequestPart::SystemPrompt { .. }
                        | ModelRequestPart::UserPrompt { .. }
                        | ModelRequestPart::Instruction { .. } => {}
                    }
                }
            }
            ModelMessage::Response(response) => {
                for part in &mut response.parts {
                    self.wrap_response_part(part);
                }
            }
        }
    }

    /// Wrap all tool call IDs in one response part in place.
    pub fn wrap_response_part(&mut self, part: &mut ModelResponsePart) {
        match part {
            ModelResponsePart::ToolCall(call)
            | ModelResponsePart::ProviderToolCall { call, .. } => {
                call.id = self.upsert_tool_call_id(&call.id);
            }
            ModelResponsePart::NativeToolCall { payload, .. }
            | ModelResponsePart::NativeToolReturn { payload, .. }
            | ModelResponsePart::ProviderOpaque { payload, .. } => {
                self.wrap_provider_payload(payload);
            }
            ModelResponsePart::Text { .. }
            | ModelResponsePart::ProviderText { .. }
            | ModelResponsePart::Thinking { .. }
            | ModelResponsePart::ProviderThinking { .. }
            | ModelResponsePart::File { .. }
            | ModelResponsePart::Compaction { .. } => {}
        }
    }

    /// Wrap tool call IDs in provider-native payload objects when they use common id keys.
    pub fn wrap_provider_payload(&mut self, payload: &mut Value) {
        let Some(object) = payload.as_object_mut() else {
            return;
        };
        for key in ["tool_call_id", "toolUseId", "tool_use_id", "call_id", "id"] {
            let Some(value) = object.get_mut(key) else {
                continue;
            };
            let Some(id) = value.as_str().map(ToString::to_string) else {
                continue;
            };
            *value = Value::String(self.upsert_tool_call_id(&id));
        }
    }

    /// Return whether no mappings are stored.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tool_call_maps.is_empty()
    }
}

/// Runtime-only stream queue registry placeholder.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentStreamQueueRegistry {
    /// Queue names/ids known to the context. Actual async queues live outside serializable state.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub queues: Vec<String>,
}

impl AgentStreamQueueRegistry {
    /// Return whether no queues are registered.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.queues.is_empty()
    }
}

/// Loaded tool-search state.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolSearchState {
    /// Tool names loaded via tool search.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub loaded_tools: Vec<String>,
    /// Namespace IDs loaded via tool search.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub loaded_namespaces: Vec<String>,
}

impl ToolSearchState {
    /// Return whether no tool-search state is present.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.loaded_tools.is_empty() && self.loaded_namespaces.is_empty()
    }
}

/// Removed tool-search state after host invalidation or refresh.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolSearchInvalidation {
    /// Tool names removed from loaded tool-search state.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed_tools: Vec<String>,
    /// Namespace IDs removed from loaded tool-search state.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed_namespaces: Vec<String>,
}

impl ToolSearchInvalidation {
    /// Return whether no loaded tool-search entries were removed.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.removed_tools.is_empty() && self.removed_namespaces.is_empty()
    }
}

/// Runtime model wrapper placeholder. Actual wrapper functions are crate-specific dependencies.
pub type ModelWrapperMetadata = Value;

fn default_tool_id_prefix() -> String {
    "sw-tool-".to_string()
}
