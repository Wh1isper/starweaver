//! Pricing profile types and cache-aware estimation helpers.

use serde::{Deserialize, Serialize};

use crate::{PricingEstimate, Usage};

use super::ModelPricing;

/// Cache-aware static model pricing in micro USD units per million tokens.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ModelPricingDetails {
    /// Standard input-token cost in micro USD units per million tokens.
    pub input_micros_per_million_tokens: u64,
    /// Output-token cost in micro USD units per million tokens.
    pub output_micros_per_million_tokens: u64,
    /// Cache-write cost in micro USD units per million tokens, when published.
    ///
    /// When absent, cache-write tokens are charged at the standard input-token rate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_micros_per_million_tokens: Option<u64>,
    /// Cache-read or cached-input cost in micro USD units per million tokens, when published.
    ///
    /// When absent, cache-read tokens are charged at the standard input-token rate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_micros_per_million_tokens: Option<u64>,
}

impl ModelPricingDetails {
    /// Create a cache-aware pricing record with standard input and output rates.
    #[must_use]
    pub const fn new(input_micros: u64, output_micros: u64) -> Self {
        Self {
            input_micros_per_million_tokens: input_micros,
            output_micros_per_million_tokens: output_micros,
            cache_write_micros_per_million_tokens: None,
            cache_read_micros_per_million_tokens: None,
        }
    }

    /// Return the standard input/output pricing projection.
    #[must_use]
    pub const fn standard_pricing(&self) -> ModelPricing {
        ModelPricing::new(
            self.input_micros_per_million_tokens,
            self.output_micros_per_million_tokens,
        )
    }

    /// Set cache-write cost in micro USD units per million tokens.
    #[must_use]
    pub const fn with_cache_write_micros_per_million_tokens(mut self, micros: u64) -> Self {
        self.cache_write_micros_per_million_tokens = Some(micros);
        self
    }

    /// Set cache-read or cached-input cost in micro USD units per million tokens.
    #[must_use]
    pub const fn with_cache_read_micros_per_million_tokens(mut self, micros: u64) -> Self {
        self.cache_read_micros_per_million_tokens = Some(micros);
        self
    }

    /// Estimate cache-aware usage pricing in micro USD units.
    ///
    /// `Usage::input_tokens` is treated as total input tokens, including cache-write
    /// and cache-read tokens when providers report those subtotals. Provider
    /// adapters and callers should normalize usage to that inclusive shape. The
    /// estimate subtracts known cache subtotals from standard input tokens and
    /// charges them with cache-specific rates when available.
    #[must_use]
    pub const fn estimate_micros(&self, usage: &Usage) -> u64 {
        let cache_write_rate = match self.cache_write_micros_per_million_tokens {
            Some(rate) => rate,
            None => self.input_micros_per_million_tokens,
        };
        let cache_read_rate = match self.cache_read_micros_per_million_tokens {
            Some(rate) => rate,
            None => self.input_micros_per_million_tokens,
        };
        let standard_input_tokens = usage
            .input_tokens
            .saturating_sub(usage.cache_write_tokens)
            .saturating_sub(usage.cache_read_tokens);

        cost_for_tokens(standard_input_tokens, self.input_micros_per_million_tokens)
            .saturating_add(cost_for_tokens(usage.cache_write_tokens, cache_write_rate))
            .saturating_add(cost_for_tokens(usage.cache_read_tokens, cache_read_rate))
            .saturating_add(cost_for_tokens(
                usage.output_tokens,
                self.output_micros_per_million_tokens,
            ))
    }

    /// Estimate cache-aware usage pricing with this model pricing record.
    #[must_use]
    pub const fn estimate(&self, usage: &Usage) -> PricingEstimate {
        PricingEstimate::from_micros_usd(self.estimate_micros(usage))
    }
}

/// One context-length-dependent pricing tier for a model.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ModelPricingTier {
    /// Inclusive input-token threshold for this tier.
    ///
    /// `None` marks an open-ended final tier. Tier selection uses
    /// `Usage::input_tokens`, which should include cached input tokens when those
    /// tokens are part of the provider-reported input total.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_input_tokens: Option<u64>,
    /// Pricing details to use when the request input length falls in this tier.
    pub pricing: ModelPricingDetails,
}

impl ModelPricingTier {
    /// Create a pricing tier with an optional inclusive input-token threshold.
    #[must_use]
    pub const fn new(max_input_tokens: Option<u64>, pricing: ModelPricingDetails) -> Self {
        Self {
            max_input_tokens,
            pricing,
        }
    }

    /// Return whether `input_tokens` falls in this tier's threshold.
    #[must_use]
    pub const fn matches(&self, input_tokens: u64) -> bool {
        match self.max_input_tokens {
            Some(max_input_tokens) => input_tokens <= max_input_tokens,
            None => true,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ModelPricingProfileKind {
    Single(ModelPricingDetails),
    Tiered(&'static [ModelPricingTier]),
}

/// Pricing profile for either fixed-rate or context-length-tiered model pricing.
///
/// Tiered profiles should list tiers from the lowest input-token range to the
/// highest range. If usage exceeds the largest explicit threshold and no
/// open-ended tier is present, the last tier is used as the best available
/// approximation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModelPricingProfile {
    kind: ModelPricingProfileKind,
}

impl ModelPricingProfile {
    /// Create a fixed-rate pricing profile.
    #[must_use]
    pub const fn from_details(pricing: ModelPricingDetails) -> Self {
        Self {
            kind: ModelPricingProfileKind::Single(pricing),
        }
    }

    /// Create a context-length-tiered pricing profile.
    #[must_use]
    pub const fn from_tiers(tiers: &'static [ModelPricingTier]) -> Self {
        Self {
            kind: ModelPricingProfileKind::Tiered(tiers),
        }
    }

    /// Return the default details used for compatibility projections.
    ///
    /// For tiered profiles this is the first, lowest-context tier.
    #[must_use]
    pub fn default_details(&self) -> ModelPricingDetails {
        match self.kind {
            ModelPricingProfileKind::Single(pricing) => pricing,
            ModelPricingProfileKind::Tiered(tiers) => tiers
                .first()
                .map_or_else(ModelPricingDetails::default, |tier| tier.pricing),
        }
    }

    /// Return whether this profile has context-length-dependent tiers.
    #[must_use]
    pub const fn is_tiered(&self) -> bool {
        matches!(self.kind, ModelPricingProfileKind::Tiered(_))
    }

    /// Return the context-length-dependent tiers, if this is a tiered profile.
    #[must_use]
    pub const fn tiers(&self) -> Option<&'static [ModelPricingTier]> {
        match self.kind {
            ModelPricingProfileKind::Single(_) => None,
            ModelPricingProfileKind::Tiered(tiers) => Some(tiers),
        }
    }

    /// Return the standard input/output compatibility projection.
    ///
    /// For tiered profiles this is the first, lowest-context tier.
    #[must_use]
    pub fn standard_pricing(&self) -> ModelPricing {
        self.default_details().standard_pricing()
    }

    /// Return the pricing details selected for a request input-token count.
    #[must_use]
    pub fn details_for_input_tokens(&self, input_tokens: u64) -> ModelPricingDetails {
        match self.kind {
            ModelPricingProfileKind::Single(pricing) => pricing,
            ModelPricingProfileKind::Tiered(tiers) => {
                let mut fallback = ModelPricingDetails::default();
                for tier in tiers {
                    fallback = tier.pricing;
                    if tier.matches(input_tokens) {
                        return tier.pricing;
                    }
                }
                fallback
            }
        }
    }

    /// Return the pricing details selected for a usage value.
    #[must_use]
    pub fn details_for_usage(&self, usage: &Usage) -> ModelPricingDetails {
        self.details_for_input_tokens(usage.input_tokens)
    }

    /// Estimate usage pricing in micro USD units with tier selection.
    ///
    /// For tiered providers, pass one provider request's usage. To estimate a
    /// cumulative run, sum per-request estimates instead of re-estimating the
    /// cumulative token total, because tiers are selected from request context
    /// length rather than run lifetime usage.
    #[must_use]
    pub fn estimate_micros(&self, usage: &Usage) -> u64 {
        self.details_for_usage(usage).estimate_micros(usage)
    }

    /// Estimate usage pricing with tier selection.
    #[must_use]
    pub fn estimate(&self, usage: &Usage) -> PricingEstimate {
        PricingEstimate::from_micros_usd(self.estimate_micros(usage))
    }
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

const CNY_PER_USD_MILLIS: u64 = 7_200;

#[allow(clippy::cast_possible_truncation)]
pub(crate) const fn cny_millis_to_usd_micros(cny_millis: u64) -> u64 {
    let micros = (cny_millis as u128)
        .saturating_mul(1_000_000)
        .saturating_add((CNY_PER_USD_MILLIS / 2) as u128)
        / CNY_PER_USD_MILLIS as u128;
    if micros > u64::MAX as u128 {
        u64::MAX
    } else {
        micros as u64
    }
}

#[allow(clippy::cast_possible_truncation)]
pub(crate) const fn scale_rate(rate: u64, numerator: u64, denominator: u64) -> u64 {
    let scaled = (rate as u128)
        .saturating_mul(numerator as u128)
        .saturating_add((denominator - 1) as u128)
        / denominator as u128;
    if scaled > u64::MAX as u128 {
        u64::MAX
    } else {
        scaled as u64
    }
}
