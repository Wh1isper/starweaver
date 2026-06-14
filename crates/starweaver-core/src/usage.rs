use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Token and request usage accumulated by model and runtime layers.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Usage {
    /// Number of provider requests.
    pub requests: u64,
    /// Input or prompt tokens.
    pub input_tokens: u64,
    /// Tokens written to a provider prompt cache.
    #[serde(default)]
    pub cache_write_tokens: u64,
    /// Tokens read from a provider prompt cache.
    #[serde(default)]
    pub cache_read_tokens: u64,
    /// Output or completion tokens.
    pub output_tokens: u64,
    /// Total tokens.
    pub total_tokens: u64,
    /// Number of successful function tool calls executed by the runtime.
    #[serde(default)]
    pub tool_calls: u64,
}

impl Usage {
    /// Add another usage value into this one.
    pub fn add_assign(&mut self, other: &Self) {
        self.requests += other.requests;
        self.input_tokens += other.input_tokens;
        self.cache_write_tokens += other.cache_write_tokens;
        self.cache_read_tokens += other.cache_read_tokens;
        self.output_tokens += other.output_tokens;
        self.total_tokens += other.total_tokens;
        self.tool_calls += other.tool_calls;
    }

    /// Return whether no usage has been recorded.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.requests == 0
            && self.input_tokens == 0
            && self.cache_write_tokens == 0
            && self.cache_read_tokens == 0
            && self.output_tokens == 0
            && self.total_tokens == 0
            && self.tool_calls == 0
    }

    /// Return a copy with additional successful tool calls applied.
    #[must_use]
    pub const fn with_additional_tool_calls(mut self, tool_calls: u64) -> Self {
        self.tool_calls = self.tool_calls.saturating_add(tool_calls);
        self
    }
}

/// Cumulative usage for one agent or usage source in the current run.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct UsageSnapshotEntry {
    /// Agent or source instance that generated this usage.
    pub agent_id: String,
    /// Human-readable agent or source name.
    pub agent_name: String,
    /// Model identifier that generated this usage.
    pub model_id: String,
    /// Cumulative token usage for this agent/source in the run.
    pub usage: Usage,
    /// Stable usage record id for idempotent updates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_id: Option<String>,
    /// Component that reported this usage.
    #[serde(default = "default_usage_source")]
    pub source: String,
}

/// Cumulative usage grouped by agent/source.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct UsageAgentTotal {
    /// Human-readable agent or source name.
    pub agent_name: String,
    /// Model identifier, or `multiple` when a source used more than one model.
    pub model_id: String,
    /// Cumulative token usage for this agent/source.
    pub usage: Usage,
    /// Stable usage record id when all grouped entries share one id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_id: Option<String>,
    /// Component that reported this usage.
    #[serde(default = "default_usage_source")]
    pub source: String,
}

/// Cumulative usage snapshot for one run.
///
/// Realtime consumers should treat each snapshot as a replacement for the
/// previous snapshot with the same run id.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct UsageSnapshot {
    /// Run identifier for the snapshot.
    pub run_id: String,
    /// Cumulative usage across all known entries in this run.
    #[serde(default)]
    pub total_usage: Usage,
    /// Per-agent/source cumulative usage entries.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<UsageSnapshotEntry>,
    /// Cumulative usage grouped by agent id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub agent_usages: BTreeMap<String, UsageAgentTotal>,
    /// Cumulative usage grouped by model id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub model_usages: BTreeMap<String, Usage>,
}

fn default_usage_source() -> String {
    "model_request".to_string()
}
