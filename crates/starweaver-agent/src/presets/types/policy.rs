use serde::{Deserialize, Serialize};
use starweaver_runtime::AgentRuntimePolicy;

use super::is_false;

/// Approval policy preset for tools and host operations.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ApprovalPolicyPreset {
    /// Tool names requiring approval.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub approval_required_tools: Vec<String>,
    /// Tool names using deferred execution.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deferred_tools: Vec<String>,
    /// Whether network tools require approval.
    #[serde(default, skip_serializing_if = "is_false")]
    pub network_requires_approval: bool,
}

/// Retry and timeout policy preset.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct RetryPolicyPreset {
    /// Maximum model/tool loop steps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_steps: Option<usize>,
    /// Output validation retry budget.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_retries: Option<usize>,
    /// Agent-level function tool retry budget.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_retries: Option<usize>,
    /// Optional timeout in milliseconds for future host adapters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

impl RetryPolicyPreset {
    pub(in crate::presets) fn merge(&mut self, overlay: &Self) {
        if overlay.max_steps.is_some() {
            self.max_steps = overlay.max_steps;
        }
        if overlay.output_retries.is_some() {
            self.output_retries = overlay.output_retries;
        }
        if overlay.tool_retries.is_some() {
            self.tool_retries = overlay.tool_retries;
        }
        if overlay.timeout_ms.is_some() {
            self.timeout_ms = overlay.timeout_ms;
        }
    }

    pub(in crate::presets) const fn apply_runtime(&self, runtime: &mut AgentRuntimePolicy) {
        if let Some(max_steps) = self.max_steps {
            runtime.max_steps = max_steps;
        }
        if let Some(output_retries) = self.output_retries {
            runtime.output_retries = output_retries;
        }
    }
}

/// Streaming policy preset.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct StreamingPolicyPreset {
    /// Whether stream events should be collected by default.
    #[serde(default, skip_serializing_if = "is_false")]
    pub collect_events: bool,
    /// Stable host stream adapter name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter: Option<String>,
    /// Whether stream replay metadata should be persisted.
    #[serde(default, skip_serializing_if = "is_false")]
    pub replay: bool,
}

/// Observability policy preset.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct ObservabilityPolicyPreset {
    /// Whether tracing is enabled.
    #[serde(default, skip_serializing_if = "is_false")]
    pub trace_enabled: bool,
    /// Optional exporter name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exporter: Option<String>,
    /// Sensitive keys or paths to redact.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub redaction_keys: Vec<String>,
    /// Optional sampling ratio.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sampling_ratio: Option<f64>,
}

/// Environment policy preset.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct EnvironmentPolicyPreset {
    /// Stable environment provider or profile name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Filesystem roots or logical mount names.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roots: Vec<String>,
    /// Whether process-capable shell support is requested.
    #[serde(default, skip_serializing_if = "is_false")]
    pub process_capable: bool,
    /// Whether sandbox support is requested.
    #[serde(default, skip_serializing_if = "is_false")]
    pub sandbox: bool,
}

/// Durability policy preset.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct DurabilityPolicyPreset {
    /// Stable session store name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_store: Option<String>,
    /// Checkpoint cadence in runtime loop steps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint_every_steps: Option<usize>,
    /// Whether stream records are persisted.
    #[serde(default, skip_serializing_if = "is_false")]
    pub persist_streams: bool,
    /// Whether resume profiles are enabled.
    #[serde(default, skip_serializing_if = "is_false")]
    pub resume_enabled: bool,
}
