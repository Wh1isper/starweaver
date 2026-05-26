//! Provider-neutral generation settings.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Per-request generation configuration.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct ModelSettings {
    /// Maximum generated tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// Sampling temperature.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    /// Nucleus sampling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    /// Top-k sampling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    /// Request timeout in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// Allow multiple tool calls in one response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallel_tool_calls: Option<bool>,
    /// Tool forcing or availability policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
    /// Best-effort deterministic seed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    /// Stop strings.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stop_sequences: Vec<String>,
    /// Presence penalty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f64>,
    /// Frequency penalty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f64>,
    /// Reasoning or thinking controls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingSettings>,
    /// Latency/cost tier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<ServiceTier>,
    /// Provider-specific settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_options: Option<Value>,
    /// Request headers merged after adapter defaults and before request-level overrides.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra_headers: BTreeMap<String, String>,
    /// Extra JSON object merged into the top-level request body.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub extra_body: Map<String, Value>,
}

impl ModelSettings {
    /// Merge two settings values field by field, taking values from `overlay` when present.
    #[must_use]
    pub fn merge(&self, overlay: &Self) -> Self {
        Self {
            max_tokens: overlay.max_tokens.or(self.max_tokens),
            temperature: overlay.temperature.or(self.temperature),
            top_p: overlay.top_p.or(self.top_p),
            top_k: overlay.top_k.or(self.top_k),
            timeout_ms: overlay.timeout_ms.or(self.timeout_ms),
            parallel_tool_calls: overlay.parallel_tool_calls.or(self.parallel_tool_calls),
            tool_choice: overlay
                .tool_choice
                .clone()
                .or_else(|| self.tool_choice.clone()),
            seed: overlay.seed.or(self.seed),
            stop_sequences: if overlay.stop_sequences.is_empty() {
                self.stop_sequences.clone()
            } else {
                overlay.stop_sequences.clone()
            },
            presence_penalty: overlay.presence_penalty.or(self.presence_penalty),
            frequency_penalty: overlay.frequency_penalty.or(self.frequency_penalty),
            thinking: overlay.thinking.clone().or_else(|| self.thinking.clone()),
            service_tier: overlay
                .service_tier
                .clone()
                .or_else(|| self.service_tier.clone()),
            provider_options: overlay
                .provider_options
                .clone()
                .or_else(|| self.provider_options.clone()),
            extra_headers: if overlay.extra_headers.is_empty() {
                self.extra_headers.clone()
            } else {
                let mut headers = self.extra_headers.clone();
                headers.extend(overlay.extra_headers.clone());
                headers
            },
            extra_body: if overlay.extra_body.is_empty() {
                self.extra_body.clone()
            } else {
                let mut body = self.extra_body.clone();
                body.extend(overlay.extra_body.clone());
                body
            },
        }
    }
}

/// Tool selection policy.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ToolChoice {
    /// Provider decides whether to call a tool.
    Auto,
    /// Disable tools.
    None,
    /// Require any tool call.
    Required,
    /// Force a named tool.
    Tool {
        /// Tool name.
        name: String,
    },
}

/// Reasoning or thinking controls.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ThinkingSettings {
    /// Effort level such as low, medium, or high.
    pub effort: String,
    /// Optional token budget.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_tokens: Option<u32>,
}

/// Provider service tier.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceTier {
    /// Default provider tier.
    Auto,
    /// Low-latency tier.
    Flex,
    /// Priority tier.
    Priority,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_overlay_fields() {
        let base = ModelSettings {
            max_tokens: Some(128),
            temperature: Some(0.2),
            stop_sequences: vec!["base".to_string()],
            ..ModelSettings::default()
        };
        let overlay = ModelSettings {
            temperature: Some(0.7),
            stop_sequences: vec!["overlay".to_string()],
            ..ModelSettings::default()
        };

        let merged = base.merge(&overlay);

        assert_eq!(merged.max_tokens, Some(128));
        assert_eq!(merged.temperature, Some(0.7));
        assert_eq!(merged.stop_sequences, vec!["overlay"]);
    }
}
