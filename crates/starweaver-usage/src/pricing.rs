//! Optional USD pricing helpers for usage accounting.
//!
//! Built-in model prices are best-effort snapshots of public standard direct API
//! pricing checked on 2026-06-16. They intentionally exclude batch discounts,
//! priority/flex tiers, regional multipliers, audio/image modality surcharges,
//! cache storage charges, taxes, promotions, and enterprise contracts unless the
//! price can be represented by per-token input, cache-write, cache-read, output,
//! and context-length tiers.
//!
//! Providers that publish only CNY prices are represented as approximate USD
//! estimates using a fixed `7.2 CNY = 1 USD` conversion snapshot. Use
//! [`CostBudget`] when an application needs exact billing terms.

mod catalog;
pub mod profile;

use serde::{Deserialize, Serialize};

use crate::{PricingEstimate, Usage, UsageLimitError};

pub use profile::{ModelPricingDetails, ModelPricingProfile, ModelPricingTier};

use profile::cost_for_tokens;

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
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
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
    ///
    /// This preserves the historical standard-input/output behavior. Use
    /// [`ModelPricingDetails::estimate`] for cache-aware estimates.
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

/// Return the built-in best-effort pricing profile for a known model id.
///
/// The returned profile exposes context-length tiers when the catalog has them.
#[must_use]
pub fn known_model_pricing_profile(model_id: &str) -> Option<ModelPricingProfile> {
    catalog::lookup_model_pricing_profile(model_id)
}

/// Return built-in best-effort pricing details for a known model id.
///
/// For context-length-tiered providers this returns the default/lowest-context
/// tier for compatibility with callers that expect a single cache-aware record.
#[must_use]
pub fn known_model_pricing_details(model_id: &str) -> Option<ModelPricingDetails> {
    known_model_pricing_profile(model_id).map(|profile| profile.default_details())
}

/// Return built-in best-effort standard input/output pricing for a known model id.
///
/// For context-length-tiered providers this is the compatibility projection of
/// the default/lowest standard input/output rate.
#[must_use]
pub fn known_model_pricing(model_id: &str) -> Option<ModelPricing> {
    known_model_pricing_profile(model_id).map(|profile| profile.standard_pricing())
}

/// Estimate USD pricing for a known model id.
///
/// Uses cache-aware pricing and selects context-length-dependent tiers when the
/// built-in catalog has them for the requested model. For tiered providers, pass
/// one provider request's usage; cumulative run estimates should sum per-request
/// estimates instead of re-estimating a lifetime token total.
#[must_use]
pub fn estimate_pricing_for_model(model_id: &str, usage: &Usage) -> Option<PricingEstimate> {
    known_model_pricing_profile(model_id).map(|profile| profile.estimate(usage))
}

fn normalize_model_id(model_id: &str) -> String {
    let lower = model_id.trim().to_ascii_lowercase();
    let after_colon = lower
        .rsplit_once(':')
        .map_or_else(|| lower.as_str(), |(_, model)| model);
    after_colon
        .rsplit_once('/')
        .map_or(after_colon, |(_, model)| model)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use profile::{cny_millis_to_usd_micros, cost_for_tokens};

    const ONE_MILLION_USAGE: Usage = Usage {
        requests: 1,
        input_tokens: 1_000_000,
        cache_write_tokens: 0,
        cache_read_tokens: 0,
        output_tokens: 1_000_000,
        total_tokens: 2_000_000,
        tool_calls: 0,
    };

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
        assert_eq!(
            estimate_pricing_for_model("openai:gpt-4o-mini", &ONE_MILLION_USAGE)
                .map(|estimate| estimate.amount_micros_usd),
            Some(750_000)
        );
    }

    #[test]
    fn known_model_pricing_normalizes_prefixes_namespaces_case_and_whitespace() {
        assert_eq!(
            known_model_pricing("  openrouter:openai/GPT-4O-MINI  "),
            known_model_pricing("gpt-4o-mini")
        );
        assert_eq!(
            known_model_pricing("models/gemini-2.5-flash"),
            Some(ModelPricing::new(300_000, 2_500_000))
        );
        assert_eq!(
            known_model_pricing("Google/Gemini-2-5-Pro"),
            known_model_pricing("gemini-2.5-pro")
        );
    }

    #[test]
    fn known_model_pricing_covers_requested_provider_families() {
        let expected = [
            ("glm-5.1", ModelPricing::new(1_400_000, 4_400_000)),
            ("minimax-m3", ModelPricing::new(300_000, 1_200_000)),
            (
                "qwen-plus",
                ModelPricing::new(
                    cny_millis_to_usd_micros(800),
                    cny_millis_to_usd_micros(2_000),
                ),
            ),
            ("claude-sonnet-4", ModelPricing::new(3_000_000, 15_000_000)),
            ("gpt-4.1", ModelPricing::new(2_000_000, 8_000_000)),
            ("gemini-2.5-flash", ModelPricing::new(300_000, 2_500_000)),
            (
                "gemini-3-flash-preview",
                ModelPricing::new(500_000, 3_000_000),
            ),
            (
                "gemini-3.1-flash-lite",
                ModelPricing::new(250_000, 1_500_000),
            ),
            (
                "gemini-3.1-pro-preview",
                ModelPricing::new(2_000_000, 12_000_000),
            ),
            ("gemini-3.5-flash", ModelPricing::new(1_500_000, 9_000_000)),
            ("kimi-k2.7-code", ModelPricing::new(950_000, 4_000_000)),
            ("mimo-v2.5", ModelPricing::new(140_000, 280_000)),
            ("deepseek-v4-pro", ModelPricing::new(435_000, 870_000)),
        ];

        for (model_id, pricing) in expected {
            assert_eq!(known_model_pricing(model_id), Some(pricing), "{model_id}");
        }
    }

    #[test]
    fn known_model_pricing_covers_gemini_3_family() {
        let expected = [
            ("gemini-3.5-flash", ModelPricing::new(1_500_000, 9_000_000)),
            (
                "gemini-3.5-live-translate-preview",
                ModelPricing::new(3_500_000, 21_000_000),
            ),
            (
                "gemini-3.1-flash-lite",
                ModelPricing::new(250_000, 1_500_000),
            ),
            (
                "gemini-3.1-pro-preview",
                ModelPricing::new(2_000_000, 12_000_000),
            ),
            (
                "gemini-3.1-pro-preview-customtools",
                ModelPricing::new(2_000_000, 12_000_000),
            ),
            (
                "gemini-3.1-flash-live-preview",
                ModelPricing::new(750_000, 4_500_000),
            ),
            (
                "gemini-3.1-flash-image",
                ModelPricing::new(500_000, 3_000_000),
            ),
            (
                "gemini-3.1-flash-tts-preview",
                ModelPricing::new(1_000_000, 20_000_000),
            ),
            (
                "gemini-3-flash-preview",
                ModelPricing::new(500_000, 3_000_000),
            ),
            (
                "gemini-3-pro-image",
                ModelPricing::new(2_000_000, 12_000_000),
            ),
        ];

        for (model_id, pricing) in expected {
            assert_eq!(known_model_pricing(model_id), Some(pricing), "{model_id}");
        }
        assert_eq!(
            known_model_pricing("models/gemini-3-1-pro-preview"),
            known_model_pricing("gemini-3.1-pro-preview")
        );
        assert_eq!(
            known_model_pricing_profile("gemini-3.1-pro-preview")
                .map(|profile| profile.details_for_input_tokens(200_001).standard_pricing()),
            Some(ModelPricing::new(4_000_000, 18_000_000))
        );
    }

    #[test]
    fn estimate_pricing_for_model_uses_cache_read_rates() {
        let usage = Usage {
            requests: 1,
            input_tokens: 1_000_000,
            cache_write_tokens: 0,
            cache_read_tokens: 250_000,
            output_tokens: 1_000_000,
            total_tokens: 2_000_000,
            tool_calls: 0,
        };

        assert_eq!(
            estimate_pricing_for_model("gpt-4.1", &usage)
                .map(|estimate| estimate.amount_micros_usd),
            Some(9_625_000)
        );
    }

    #[test]
    fn pricing_details_estimate_uses_cache_write_and_read_rates() {
        let usage = Usage {
            requests: 1,
            input_tokens: 1_000_000,
            cache_write_tokens: 200_000,
            cache_read_tokens: 300_000,
            output_tokens: 1_000_000,
            total_tokens: 2_000_000,
            tool_calls: 0,
        };

        assert_eq!(
            estimate_pricing_for_model("claude-sonnet-4", &usage)
                .map(|estimate| estimate.amount_micros_usd),
            Some(17_340_000)
        );
    }

    #[test]
    fn gemini_2_5_pro_uses_over_200k_tier() {
        let usage = Usage {
            requests: 1,
            input_tokens: 200_001,
            cache_write_tokens: 0,
            cache_read_tokens: 50_000,
            output_tokens: 100_000,
            total_tokens: 300_001,
            tool_calls: 0,
        };

        assert_eq!(
            known_model_pricing("gemini-2.5-pro"),
            Some(ModelPricing::new(1_250_000, 10_000_000))
        );
        assert_eq!(
            estimate_pricing_for_model("gemini-2.5-pro", &usage)
                .map(|estimate| estimate.amount_micros_usd),
            Some(1_887_503)
        );
    }

    #[test]
    fn minimax_m3_uses_over_512k_tier() {
        let usage = Usage {
            requests: 1,
            input_tokens: 512_001,
            cache_write_tokens: 0,
            cache_read_tokens: 100_000,
            output_tokens: 100_000,
            total_tokens: 612_001,
            tool_calls: 0,
        };

        assert_eq!(
            estimate_pricing_for_model("minimax-m3", &usage)
                .map(|estimate| estimate.amount_micros_usd),
            Some(499_201)
        );
    }

    #[test]
    fn qwen_tier_uses_published_token_ranges() {
        let usage = Usage {
            requests: 1,
            input_tokens: 33_000,
            cache_write_tokens: 0,
            cache_read_tokens: 3_000,
            output_tokens: 10_000,
            total_tokens: 43_000,
            tool_calls: 0,
        };

        assert_eq!(
            known_model_pricing("qwen3-max"),
            Some(ModelPricing::new(
                cny_millis_to_usd_micros(2_500),
                cny_millis_to_usd_micros(10_000),
            ))
        );
        assert_eq!(
            estimate_pricing_for_model("qwen3-max", &usage)
                .map(|estimate| estimate.amount_micros_usd),
            Some(39_057)
        );
    }

    #[test]
    fn model_pricing_details_serde_defaults_are_backwards_compatible() {
        let decoded = serde_json::from_str::<ModelPricingDetails>(
            r#"{"input_micros_per_million_tokens":1,"output_micros_per_million_tokens":2}"#,
        )
        .ok();

        assert_eq!(decoded, Some(ModelPricingDetails::new(1, 2)));
    }

    #[test]
    fn unknown_model_pricing_returns_none() {
        assert_eq!(known_model_pricing("unknown-model"), None);
        assert_eq!(known_model_pricing_profile("unknown-model"), None);
        assert_eq!(known_model_pricing_details("unknown-model"), None);
        assert_eq!(
            estimate_pricing_for_model("unknown-model", &ONE_MILLION_USAGE),
            None
        );
    }

    #[test]
    fn pricing_profile_exposes_tiers() {
        let Some(profile) = known_model_pricing_profile("gemini-2.5-pro") else {
            panic!("gemini-2.5-pro pricing profile should exist");
        };

        assert!(profile.is_tiered());
        assert_eq!(profile.tiers().map(<[ModelPricingTier]>::len), Some(2));
        assert_eq!(
            profile.details_for_input_tokens(200_001).standard_pricing(),
            ModelPricing::new(2_500_000, 15_000_000)
        );
    }

    #[test]
    fn token_cost_estimation_clamps_after_wide_arithmetic() {
        assert_eq!(cost_for_tokens(u64::MAX, u64::MAX), u64::MAX);
    }
}
