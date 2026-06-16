//! Optional USD pricing helpers for usage accounting.

use serde::{Deserialize, Serialize};

use crate::{PricingEstimate, Usage, UsageLimitError};

/// Cost budget derived from usage and caller-provided USD pricing.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct CostBudget {
    /// Input-token cost in micro USD units per million tokens.
    #[serde(default)]
    pub input_micros_per_million_tokens: u64,
    /// Output-token cost in micro USD units per million tokens.
    #[serde(default)]
    pub output_micros_per_million_tokens: u64,
    /// Fixed cost in micro USD units per provider request.
    #[serde(default)]
    pub request_micros: u64,
    /// Maximum accumulated cost in micro USD units.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_cost_limit_micros: Option<u64>,
}

impl CostBudget {
    /// Create an empty cost budget.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            input_micros_per_million_tokens: 0,
            output_micros_per_million_tokens: 0,
            request_micros: 0,
            total_cost_limit_micros: None,
        }
    }

    /// Set input-token cost in micro USD units per million tokens.
    #[must_use]
    pub const fn with_input_micros_per_million_tokens(mut self, micros: u64) -> Self {
        self.input_micros_per_million_tokens = micros;
        self
    }

    /// Set output-token cost in micro USD units per million tokens.
    #[must_use]
    pub const fn with_output_micros_per_million_tokens(mut self, micros: u64) -> Self {
        self.output_micros_per_million_tokens = micros;
        self
    }

    /// Set fixed cost in micro USD units per provider request.
    #[must_use]
    pub const fn with_request_micros(mut self, micros: u64) -> Self {
        self.request_micros = micros;
        self
    }

    /// Set accumulated cost limit in micro USD units.
    #[must_use]
    pub const fn with_total_cost_limit_micros(mut self, micros: u64) -> Self {
        self.total_cost_limit_micros = Some(micros);
        self
    }

    /// Estimate cost from accumulated usage in micro USD units.
    #[must_use]
    pub const fn estimate_micros(&self, usage: &Usage) -> u64 {
        usage
            .requests
            .saturating_mul(self.request_micros)
            .saturating_add(cost_for_tokens(
                usage.input_tokens,
                self.input_micros_per_million_tokens,
            ))
            .saturating_add(cost_for_tokens(
                usage.output_tokens,
                self.output_micros_per_million_tokens,
            ))
    }

    /// Estimate cost from accumulated usage as a USD pricing estimate.
    #[must_use]
    pub const fn estimate_pricing(&self, usage: &Usage) -> PricingEstimate {
        PricingEstimate::from_micros_usd(self.estimate_micros(usage))
    }

    /// Check whether accumulated usage exceeds the configured cost budget.
    ///
    /// # Errors
    ///
    /// Returns an error when estimated accumulated cost exceeds the configured limit.
    pub const fn check_usage(&self, usage: &Usage) -> Result<(), UsageLimitError> {
        if let Some(limit) = self.total_cost_limit_micros {
            let actual = self.estimate_micros(usage);
            if actual > limit {
                return Err(UsageLimitError::Cost {
                    limit_micros: limit,
                    actual_micros: actual,
                });
            }
        }
        Ok(())
    }
}

/// Static model pricing in micro USD units per million tokens.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ModelPricing {
    /// Input-token cost in micro USD units per million tokens.
    pub input_micros_per_million_tokens: u64,
    /// Output-token cost in micro USD units per million tokens.
    pub output_micros_per_million_tokens: u64,
}

impl ModelPricing {
    /// Create a pricing record from USD-per-million-token values encoded as micro USD.
    #[must_use]
    pub const fn new(input_micros: u64, output_micros: u64) -> Self {
        Self {
            input_micros_per_million_tokens: input_micros,
            output_micros_per_million_tokens: output_micros,
        }
    }

    /// Estimate usage pricing with this model pricing record.
    #[must_use]
    pub const fn estimate(&self, usage: &Usage) -> PricingEstimate {
        PricingEstimate::from_micros_usd(
            cost_for_tokens(usage.input_tokens, self.input_micros_per_million_tokens)
                .saturating_add(cost_for_tokens(
                    usage.output_tokens,
                    self.output_micros_per_million_tokens,
                )),
        )
    }
}

/// Return built-in best-effort pricing for a known model id.
#[must_use]
pub fn known_model_pricing(model_id: &str) -> Option<ModelPricing> {
    let normalized = normalize_model_id(model_id);
    match normalized.as_str() {
        "gpt-4o" | "chatgpt-4o-latest" => Some(ModelPricing::new(2_500_000, 10_000_000)),
        "gpt-4o-mini" => Some(ModelPricing::new(150_000, 600_000)),
        "gpt-4.1" => Some(ModelPricing::new(2_000_000, 8_000_000)),
        "gpt-4.1-mini" => Some(ModelPricing::new(400_000, 1_600_000)),
        "gpt-4.1-nano" | "gemini-2.0-flash" | "gemini-2.0-flash-001" => {
            Some(ModelPricing::new(100_000, 400_000))
        }
        "claude-3-5-sonnet-latest"
        | "claude-3-5-sonnet-20241022"
        | "claude-sonnet-4"
        | "claude-sonnet-4-20250514" => Some(ModelPricing::new(3_000_000, 15_000_000)),
        "claude-3-5-haiku-latest" | "claude-3-5-haiku-20241022" => {
            Some(ModelPricing::new(800_000, 4_000_000))
        }
        "claude-opus-4" | "claude-opus-4-20250514" => {
            Some(ModelPricing::new(15_000_000, 75_000_000))
        }
        "gemini-1.5-flash" | "gemini-1.5-flash-latest" => Some(ModelPricing::new(75_000, 300_000)),
        "gemini-1.5-pro" | "gemini-1.5-pro-latest" => Some(ModelPricing::new(1_250_000, 5_000_000)),
        _ => None,
    }
}

/// Estimate USD pricing for a known model id.
#[must_use]
pub fn estimate_pricing_for_model(model_id: &str, usage: &Usage) -> Option<PricingEstimate> {
    known_model_pricing(model_id).map(|pricing| pricing.estimate(usage))
}

fn normalize_model_id(model_id: &str) -> String {
    let lower = model_id.trim().to_ascii_lowercase();
    lower
        .rsplit_once(':')
        .map_or_else(|| lower.clone(), |(_, model)| model.to_string())
}

#[allow(clippy::cast_possible_truncation)]
pub(crate) const fn cost_for_tokens(tokens: u64, micros_per_million_tokens: u64) -> u64 {
    let cost = (tokens as u128)
        .saturating_mul(micros_per_million_tokens as u128)
        .saturating_add(999_999)
        / 1_000_000;
    if cost > u64::MAX as u128 {
        u64::MAX
    } else {
        cost as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cost_budget_estimates_usage_cost_in_micros() {
        let budget = CostBudget::new()
            .with_request_micros(100)
            .with_input_micros_per_million_tokens(1_000_000)
            .with_output_micros_per_million_tokens(2_000_000)
            .with_total_cost_limit_micros(1_000);
        let usage = Usage {
            requests: 2,
            input_tokens: 10,
            cache_write_tokens: 0,
            cache_read_tokens: 0,
            output_tokens: 20,
            total_tokens: 30,
            tool_calls: 0,
        };

        assert_eq!(budget.estimate_micros(&usage), 250);
        assert_eq!(budget.estimate_pricing(&usage).amount_micros_usd, 250);
    }

    #[test]
    fn known_model_pricing_estimates_prefixed_model_ids() {
        let usage = Usage {
            requests: 1,
            input_tokens: 1_000_000,
            cache_write_tokens: 0,
            cache_read_tokens: 0,
            output_tokens: 1_000_000,
            total_tokens: 2_000_000,
            tool_calls: 0,
        };
        assert_eq!(
            estimate_pricing_for_model("openai:gpt-4o-mini", &usage)
                .map(|estimate| estimate.amount_micros_usd),
            Some(750_000)
        );
    }

    #[test]
    fn token_cost_estimation_clamps_after_wide_arithmetic() {
        assert_eq!(cost_for_tokens(u64::MAX, u64::MAX), u64::MAX);
    }
}
