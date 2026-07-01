//! Usage accounting, limits, and optional pricing primitives for Starweaver.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[cfg(feature = "pricing")]
pub mod pricing;

/// Token and request usage accumulated by model and runtime layers.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Usage {
    /// Number of provider requests.
    pub requests: u64,
    /// Input or prompt tokens.
    ///
    /// Provider adapters should normalize this as total provider-billed input
    /// tokens for the request, including cache-write and cache-read tokens when
    /// those subtotals are present. Pricing helpers subtract the cache subtotals
    /// before applying cache-specific rates.
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
    pub const fn add_assign(&mut self, other: &Self) {
        self.requests = self.requests.saturating_add(other.requests);
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.cache_write_tokens = self
            .cache_write_tokens
            .saturating_add(other.cache_write_tokens);
        self.cache_read_tokens = self
            .cache_read_tokens
            .saturating_add(other.cache_read_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.total_tokens = self.total_tokens.saturating_add(other.total_tokens);
        self.tool_calls = self.tool_calls.saturating_add(other.tool_calls);
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

/// Estimated USD pricing for usage.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Ord, PartialOrd, Serialize)]
pub struct PricingEstimate {
    /// Estimated cost in micro USD units.
    #[serde(default)]
    pub amount_micros_usd: u64,
}

impl PricingEstimate {
    /// Create an estimate from micro USD units.
    #[must_use]
    pub const fn from_micros_usd(amount_micros_usd: u64) -> Self {
        Self { amount_micros_usd }
    }

    /// Add another estimate into this one.
    pub const fn add_assign(&mut self, other: &Self) {
        self.amount_micros_usd = self
            .amount_micros_usd
            .saturating_add(other.amount_micros_usd);
    }

    /// Return whether the estimate is zero.
    #[must_use]
    pub const fn is_zero(&self) -> bool {
        self.amount_micros_usd == 0
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
    /// Estimated cumulative pricing for this entry, in USD.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimate_pricing: Option<PricingEstimate>,
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
    /// Estimated cumulative pricing for this agent/source, in USD.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimate_pricing: Option<PricingEstimate>,
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
    /// Usage reported by the latest provider request that produced this snapshot.
    ///
    /// This is intentionally separate from `total_usage`: realtime UI surfaces may use
    /// the latest request total tokens as the current context-window estimate,
    /// while `total_usage` remains the cumulative run ledger.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_usage: Option<Usage>,
    /// Cumulative usage across all known entries in this run.
    #[serde(default)]
    pub total_usage: Usage,
    /// Estimated cumulative pricing across all known entries in this run, in USD.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimate_pricing: Option<PricingEstimate>,
    /// Per-agent/source cumulative usage entries.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<UsageSnapshotEntry>,
    /// Cumulative usage grouped by agent id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub agent_usages: BTreeMap<String, UsageAgentTotal>,
    /// Cumulative usage grouped by model id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub model_usages: BTreeMap<String, Usage>,
    /// Estimated cumulative pricing grouped by model id, in USD.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub model_estimate_pricing: BTreeMap<String, PricingEstimate>,
}

fn default_usage_source() -> String {
    "model_request".to_string()
}

/// Runtime usage limits for one agent run.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct UsageLimits {
    /// Maximum provider requests allowed in one run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_limit: Option<u64>,
    /// Maximum input tokens allowed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens_limit: Option<u64>,
    /// Maximum output tokens allowed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens_limit: Option<u64>,
    /// Maximum total tokens allowed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens_limit: Option<u64>,
    /// Maximum successful function tool calls allowed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls_limit: Option<u64>,
    /// Optional USD pricing budget based on accumulated usage.
    #[cfg(feature = "pricing")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_budget: Option<pricing::CostBudget>,
}

impl UsageLimits {
    /// Create empty limits.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            request_limit: None,
            input_tokens_limit: None,
            output_tokens_limit: None,
            total_tokens_limit: None,
            tool_calls_limit: None,
            #[cfg(feature = "pricing")]
            cost_budget: None,
        }
    }

    /// Set request limit.
    #[must_use]
    pub const fn with_request_limit(mut self, limit: u64) -> Self {
        self.request_limit = Some(limit);
        self
    }

    /// Set input token limit.
    #[must_use]
    pub const fn with_input_tokens_limit(mut self, limit: u64) -> Self {
        self.input_tokens_limit = Some(limit);
        self
    }

    /// Set output token limit.
    #[must_use]
    pub const fn with_output_tokens_limit(mut self, limit: u64) -> Self {
        self.output_tokens_limit = Some(limit);
        self
    }

    /// Set total token limit.
    #[must_use]
    pub const fn with_total_tokens_limit(mut self, limit: u64) -> Self {
        self.total_tokens_limit = Some(limit);
        self
    }

    /// Set successful tool-call limit.
    #[must_use]
    pub const fn with_tool_calls_limit(mut self, limit: u64) -> Self {
        self.tool_calls_limit = Some(limit);
        self
    }

    /// Set USD cost budget.
    #[cfg(feature = "pricing")]
    #[must_use]
    pub const fn with_cost_budget(mut self, budget: pricing::CostBudget) -> Self {
        self.cost_budget = Some(budget);
        self
    }

    /// Estimate current USD cost in micro-units when a cost budget is configured.
    #[cfg(feature = "pricing")]
    #[must_use]
    pub fn estimate_cost_micros(&self, usage: &Usage) -> Option<u64> {
        self.cost_budget
            .as_ref()
            .map(|budget| budget.estimate_micros(usage))
    }

    /// Estimate current USD pricing when a cost budget is configured.
    #[cfg(feature = "pricing")]
    #[must_use]
    pub fn estimate_pricing(&self, usage: &Usage) -> Option<PricingEstimate> {
        self.cost_budget
            .as_ref()
            .map(|budget| budget.estimate_pricing(usage))
    }

    /// Check whether the next model request would exceed the request limit.
    ///
    /// # Errors
    ///
    /// Returns an error when another request would exceed the configured request limit.
    pub const fn check_before_request(&self, current: &Usage) -> Result<(), UsageLimitError> {
        if let Some(limit) = self.request_limit {
            let next = current.requests.saturating_add(1);
            if next > limit {
                return Err(UsageLimitError::NextRequest {
                    limit,
                    next_requests: next,
                });
            }
        }
        Ok(())
    }

    /// Check whether projected tool calls would exceed the tool-call limit.
    ///
    /// # Errors
    ///
    /// Returns an error when executing the next successful tool calls would exceed the configured limit.
    pub const fn check_tool_calls(&self, projected: &Usage) -> Result<(), UsageLimitError> {
        match self.tool_calls_limit {
            Some(limit) if projected.tool_calls > limit => Err(UsageLimitError::ToolCalls {
                limit,
                tool_calls: projected.tool_calls,
            }),
            _ => Ok(()),
        }
    }

    /// Check whether accumulated usage exceeds configured token or pricing limits.
    ///
    /// # Errors
    ///
    /// Returns an error when accumulated usage exceeds any configured limit.
    pub fn check_usage(&self, usage: &Usage) -> Result<(), UsageLimitError> {
        check_limit(
            UsageTokenKind::InputTokens,
            self.input_tokens_limit,
            usage.input_tokens,
        )?;
        check_limit(
            UsageTokenKind::OutputTokens,
            self.output_tokens_limit,
            usage.output_tokens,
        )?;
        check_limit(
            UsageTokenKind::TotalTokens,
            self.total_tokens_limit,
            usage.total_tokens,
        )?;
        #[cfg(feature = "pricing")]
        if let Some(budget) = &self.cost_budget {
            budget.check_usage(usage)?;
        }
        Ok(())
    }
}

/// Token counter checked by a usage limit.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageTokenKind {
    /// Input or prompt tokens.
    InputTokens,
    /// Output or completion tokens.
    OutputTokens,
    /// Total tokens.
    TotalTokens,
}

impl std::fmt::Display for UsageTokenKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = match self {
            Self::InputTokens => "input_tokens",
            Self::OutputTokens => "output_tokens",
            Self::TotalTokens => "total_tokens",
        };
        formatter.write_str(value)
    }
}

/// Usage limit error.
#[derive(Clone, Debug, Error, Deserialize, Eq, PartialEq, Serialize)]
pub enum UsageLimitError {
    /// The next request would exceed request budget.
    #[error(
        "the next request would exceed the request_limit of {limit} (next_requests={next_requests})"
    )]
    NextRequest {
        /// Configured limit.
        limit: u64,
        /// Requests after the next request.
        next_requests: u64,
    },
    /// Accumulated usage exceeded a token budget.
    #[error("exceeded the {kind}_limit of {limit} ({kind}={actual})")]
    Token {
        /// Usage kind.
        kind: UsageTokenKind,
        /// Configured limit.
        limit: u64,
        /// Actual usage value.
        actual: u64,
    },
    /// Accumulated usage exceeded a USD pricing budget.
    #[cfg(feature = "pricing")]
    #[error("exceeded the total_cost_limit_micros of {limit_micros} (cost_micros={actual_micros})")]
    Cost {
        /// Configured cost limit in micro USD units.
        limit_micros: u64,
        /// Actual estimated cost in micro USD units.
        actual_micros: u64,
    },
    /// Projected successful function tool calls would exceed the configured budget.
    #[error(
        "the next tool call(s) would exceed the tool_calls_limit of {limit} (tool_calls={tool_calls})"
    )]
    ToolCalls {
        /// Configured tool-call limit.
        limit: u64,
        /// Projected successful tool calls.
        tool_calls: u64,
    },
}

const fn check_limit(
    kind: UsageTokenKind,
    limit: Option<u64>,
    actual: u64,
) -> Result<(), UsageLimitError> {
    match limit {
        Some(limit) if actual > limit => Err(UsageLimitError::Token {
            kind,
            limit,
            actual,
        }),
        _ => Ok(()),
    }
}

/// Aggregate an optional pricing estimate into a running total.
pub fn add_optional_pricing(
    total: &mut Option<PricingEstimate>,
    estimate: Option<&PricingEstimate>,
) {
    if let Some(estimate) = estimate {
        match total {
            Some(total) => total.add_assign(estimate),
            None => *total = Some(estimate.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_add_assign_and_empty_work() {
        let mut usage = Usage {
            requests: 1,
            input_tokens: 2,
            cache_write_tokens: 7,
            cache_read_tokens: 11,
            output_tokens: 3,
            total_tokens: 5,
            tool_calls: 1,
        };
        usage.add_assign(&Usage {
            requests: 2,
            input_tokens: 4,
            cache_write_tokens: 13,
            cache_read_tokens: 17,
            output_tokens: 6,
            total_tokens: 10,
            tool_calls: 3,
        });
        assert_eq!(usage.requests, 3);
        assert_eq!(usage.input_tokens, 6);
        assert_eq!(usage.cache_write_tokens, 20);
        assert_eq!(usage.cache_read_tokens, 28);
        assert_eq!(usage.output_tokens, 9);
        assert_eq!(usage.total_tokens, 15);
        assert_eq!(usage.tool_calls, 4);
        assert_eq!(usage.clone().with_additional_tool_calls(2).tool_calls, 6);
        assert!(Usage::default().is_empty());
        assert!(!usage.is_empty());
    }

    #[test]
    fn usage_add_assign_saturates() {
        let mut usage = Usage {
            requests: u64::MAX,
            input_tokens: u64::MAX,
            cache_write_tokens: u64::MAX,
            cache_read_tokens: u64::MAX,
            output_tokens: u64::MAX,
            total_tokens: u64::MAX,
            tool_calls: u64::MAX,
        };
        usage.add_assign(&Usage {
            requests: 1,
            input_tokens: 1,
            cache_write_tokens: 1,
            cache_read_tokens: 1,
            output_tokens: 1,
            total_tokens: 1,
            tool_calls: 1,
        });
        assert_eq!(usage.requests, u64::MAX);
        assert_eq!(usage.input_tokens, u64::MAX);
        assert_eq!(usage.cache_write_tokens, u64::MAX);
        assert_eq!(usage.cache_read_tokens, u64::MAX);
        assert_eq!(usage.output_tokens, u64::MAX);
        assert_eq!(usage.total_tokens, u64::MAX);
        assert_eq!(usage.tool_calls, u64::MAX);
    }

    #[test]
    fn usage_limit_error_token_kind_is_owned_ser_de_contract() {
        let error = UsageLimitError::Token {
            kind: UsageTokenKind::TotalTokens,
            limit: 5,
            actual: 6,
        };
        let value = match serde_json::to_value(&error) {
            Ok(value) => value,
            Err(err) => panic!("usage limit error should serialize: {err}"),
        };
        let restored: UsageLimitError = match serde_json::from_value(value) {
            Ok(restored) => restored,
            Err(err) => panic!("usage limit error should deserialize: {err}"),
        };
        assert_eq!(restored, error);
    }

    #[test]
    fn usage_snapshot_accepts_missing_pricing_fields() {
        let snapshot: UsageSnapshot = match serde_json::from_value(serde_json::json!({
            "run_id": "run_1",
            "total_usage": {
                "requests": 1,
                "input_tokens": 2,
                "output_tokens": 3,
                "total_tokens": 5
            }
        })) {
            Ok(snapshot) => snapshot,
            Err(err) => panic!("usage snapshot should deserialize: {err}"),
        };
        assert_eq!(snapshot.run_id, "run_1");
        assert!(snapshot.estimate_pricing.is_none());
        assert!(snapshot.model_estimate_pricing.is_empty());
    }
}
