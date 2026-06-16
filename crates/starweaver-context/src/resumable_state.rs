//! Serializable context state used for session restoration.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use starweaver_core::{AgentId, ConversationId, Metadata, RunId, TraceContext};
use starweaver_model::{ContentPart, ModelMessage};
use starweaver_usage::{Usage, UsageSnapshotEntry};

use crate::{AgentInfo, MessageBus, ModelConfig, SecurityConfig, StateStore, ToolConfig};

/// Baseline export profile for context restoration state.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ResumableExportMode {
    /// Curated portable export behavior for session restoration.
    #[default]
    Curated,
    /// Full Starweaver runtime state export behavior.
    Full,
}

/// Export options for context restoration state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResumableExportOptions {
    mode: ResumableExportMode,
    /// Include subagent history.
    pub include_subagent: bool,
    /// Include the usage ledger.
    pub include_usage_ledger: bool,
}

impl Default for ResumableExportOptions {
    fn default() -> Self {
        Self::curated()
    }
}

impl ResumableExportOptions {
    /// Full Starweaver runtime state export behavior.
    #[must_use]
    pub const fn full() -> Self {
        Self {
            mode: ResumableExportMode::Full,
            include_subagent: true,
            include_usage_ledger: true,
        }
    }

    /// Curated portable export behavior for session restoration.
    #[must_use]
    pub const fn curated() -> Self {
        Self {
            mode: ResumableExportMode::Curated,
            include_subagent: true,
            include_usage_ledger: false,
        }
    }

    /// Include or exclude the usage ledger.
    #[must_use]
    pub const fn with_usage_ledger(mut self, include_usage_ledger: bool) -> Self {
        self.include_usage_ledger = include_usage_ledger;
        self
    }

    /// Include or exclude subagent state.
    #[must_use]
    pub const fn with_subagent(mut self, include_subagent: bool) -> Self {
        self.include_subagent = include_subagent;
        self
    }

    /// Return whether subagent state is included.
    #[must_use]
    pub const fn include_subagent(self) -> bool {
        self.include_subagent
    }

    /// Return whether the usage ledger is included.
    #[must_use]
    pub const fn include_usage_ledger(self) -> bool {
        self.include_usage_ledger
    }

    /// Return whether Starweaver runtime extensions are included.
    #[must_use]
    pub const fn include_starweaver_extensions(self) -> bool {
        matches!(self.mode, ResumableExportMode::Full)
    }

    /// Return whether runtime config and security policy are included.
    #[must_use]
    pub const fn include_runtime_policy(self) -> bool {
        matches!(self.mode, ResumableExportMode::Full)
    }
}

/// Serializable state used to restore an agent context.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResumableState {
    /// Agent identifier.
    pub agent_id: AgentId,
    /// Current run identifier when exported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    /// Conversation identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<ConversationId>,
    /// Canonical message history.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub message_history: Vec<ModelMessage>,
    /// Serialized subagent history, keyed by agent id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub subagent_history: BTreeMap<String, Vec<ModelMessage>>,
    /// User prompt content collected for the current run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_prompts: Option<Vec<ContentPart>>,
    /// Visible assistant response immediately before the current user prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_assistant_response_reference: Option<String>,
    /// Accumulated user steering messages for compact restore.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steering_messages: Vec<String>,
    /// Rendered handoff message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff_message: Option<String>,
    /// Extra shell environment variables.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub shell_env: BTreeMap<String, String>,
    /// Metadata for deferred tool calls.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub deferred_tool_metadata: BTreeMap<String, Metadata>,
    /// Serialized agent registry.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub agent_registry: BTreeMap<String, AgentInfo>,
    /// Tool names requiring approval.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub need_user_approve_tools: Vec<String>,
    /// MCP server names requiring approval.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub need_user_approve_mcps: Vec<String>,
    /// Security-related runtime configuration.
    #[serde(default, skip_serializing_if = "SecurityConfig::is_default")]
    pub security: SecurityConfig,
    /// Files to auto-load on next request.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub auto_load_files: Vec<String>,
    /// Serialized tasks from the typed task manager, keyed by task id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tasks: BTreeMap<String, Value>,
    /// Persisted notes, keyed by note id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub notes: BTreeMap<String, String>,
    /// Tool names loaded through tool search.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_search_loaded_tools: Vec<String>,
    /// Namespace IDs loaded through tool search.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_search_loaded_namespaces: Vec<String>,

    /// Accumulated usage. Starweaver extension.
    #[serde(default, skip_serializing_if = "Usage::is_empty")]
    pub usage: Usage,
    /// Per-run cumulative usage ledger entries keyed by stable source id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub usage_snapshot_entries: BTreeMap<String, UsageSnapshotEntry>,
    /// Model/runtime configuration used for injected runtime context and tool policies.
    #[serde(default, skip_serializing_if = "ModelConfig::is_default")]
    pub model_config: ModelConfig,
    /// Tool-level configuration used by first-party and host tools.
    #[serde(default, skip_serializing_if = "ToolConfig::is_default")]
    pub tool_config: ToolConfig,
    /// Context creation time used for elapsed runtime context.
    #[serde(default = "Utc::now", skip_serializing_if = "is_default_started_at")]
    pub started_at: DateTime<Utc>,
    /// State domains. Starweaver extension.
    #[serde(default, skip_serializing_if = "StateStore::is_empty")]
    pub state: StateStore,
    /// Pending bus messages. Starweaver extension.
    #[serde(default, skip_serializing_if = "MessageBus::is_empty")]
    pub message_bus: MessageBus,
    /// Trace correlation snapshot. Starweaver extension.
    #[serde(default, skip_serializing_if = "TraceContext::is_empty")]
    pub trace_snapshot: TraceContext,
    /// Run metadata. Starweaver extension.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
    /// Extra opaque data for future-compatible restore.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, Value>,
}

const fn is_default_started_at(value: &DateTime<Utc>) -> bool {
    value.timestamp() == 0 && value.timestamp_subsec_nanos() == 0
}
