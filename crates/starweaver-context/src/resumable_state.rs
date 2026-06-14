//! Serializable context state used for session restoration.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use starweaver_core::{
    AgentId, ConversationId, Metadata, RunId, TraceContext, Usage, UsageSnapshotEntry,
};
use starweaver_model::ModelMessage;

use crate::{MessageBus, ModelConfig, NoteStore, SecurityConfig, StateStore, ToolConfig};

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
    /// Accumulated usage.
    #[serde(default)]
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
    /// Security-related runtime configuration.
    #[serde(default, skip_serializing_if = "SecurityConfig::is_default")]
    pub security: SecurityConfig,
    /// Context creation time used for elapsed runtime context.
    #[serde(default = "Utc::now")]
    pub started_at: DateTime<Utc>,
    /// State domains.
    #[serde(default)]
    pub state: StateStore,
    /// Persisted notes.
    #[serde(default, skip_serializing_if = "NoteStore::is_empty")]
    pub notes: NoteStore,
    /// Pending bus messages.
    #[serde(default)]
    pub message_bus: MessageBus,
    /// Trace correlation snapshot.
    #[serde(default, skip_serializing_if = "TraceContext::is_empty")]
    pub trace_snapshot: TraceContext,
    /// Run metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}
