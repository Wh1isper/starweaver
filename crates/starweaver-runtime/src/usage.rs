//! Usage limits for agent runs.

use serde::{Deserialize, Serialize};
use starweaver_core::Usage;
use thiserror::Error;

/// Cost budget derived from usage and caller-provided pricing.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct CostBudget {
    /// Input-token cost in micro-units per million tokens.
    #[serde(default)]
    pub input_micros_per_million_tokens: u64,
    /// Output-token cost in micro-units per million tokens.
    #[serde(default)]
    pub output_micros_per_million_tokens: u64,
    /// Fixed cost in micro-units per provider request.
    #[serde(default)]
    pub request_micros: u64,
    /// Maximum accumulated cost in micro-units.
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

    /// Set input-token cost in micro-units per million tokens.
    #[must_use]
    pub const fn with_input_micros_per_million_tokens(mut self, micros: u64) -> Self {
        self.input_micros_per_million_tokens = micros;
        self
    }

    /// Set output-token cost in micro-units per million tokens.
    #[must_use]
    pub const fn with_output_micros_per_million_tokens(mut self, micros: u64) -> Self {
        self.output_micros_per_million_tokens = micros;
        self
    }

    /// Set fixed cost in micro-units per provider request.
    #[must_use]
    pub const fn with_request_micros(mut self, micros: u64) -> Self {
        self.request_micros = micros;
        self
    }

    /// Set accumulated cost limit in micro-units.
    #[must_use]
    pub const fn with_total_cost_limit_micros(mut self, micros: u64) -> Self {
        self.total_cost_limit_micros = Some(micros);
        self
    }

    /// Estimate cost from accumulated usage.
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
    /// Optional cost budget based on accumulated usage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_budget: Option<CostBudget>,
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

    /// Set cost budget.
    #[must_use]
    pub const fn with_cost_budget(mut self, budget: CostBudget) -> Self {
        self.cost_budget = Some(budget);
        self
    }

    /// Estimate current cost in micro-units when a cost budget is configured.
    #[must_use]
    pub fn estimate_cost_micros(&self, usage: &Usage) -> Option<u64> {
        self.cost_budget
            .as_ref()
            .map(|budget| budget.estimate_micros(usage))
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
        if let Some(limit) = self.tool_calls_limit {
            if projected.tool_calls > limit {
                return Err(UsageLimitError::ToolCalls {
                    limit,
                    tool_calls: projected.tool_calls,
                });
            }
        }
        Ok(())
    }

    /// Check whether accumulated usage exceeds configured token limits.
    ///
    /// # Errors
    ///
    /// Returns an error when accumulated usage exceeds any configured token limit.
    pub fn check_usage(&self, usage: &Usage) -> Result<(), UsageLimitError> {
        check_limit("input_tokens", self.input_tokens_limit, usage.input_tokens)?;
        check_limit(
            "output_tokens",
            self.output_tokens_limit,
            usage.output_tokens,
        )?;
        check_limit("total_tokens", self.total_tokens_limit, usage.total_tokens)?;
        if let Some(budget) = &self.cost_budget {
            budget.check_usage(usage)?;
        }
        Ok(())
    }
}

/// Usage limit error.
#[derive(Clone, Debug, Error, Deserialize, Eq, PartialEq, Serialize)]
pub enum UsageLimitError {
    /// The next request would exceed request budget.
    #[error("the next request would exceed the request_limit of {limit} (next_requests={next_requests})")]
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
        kind: &'static str,
        /// Configured limit.
        limit: u64,
        /// Actual usage value.
        actual: u64,
    },
    /// Accumulated usage exceeded a cost budget.
    #[error(
        "exceeded the total_cost_limit_micros of {limit_micros} (cost_micros={actual_micros})"
    )]
    Cost {
        /// Configured cost limit in micro-units.
        limit_micros: u64,
        /// Actual estimated cost in micro-units.
        actual_micros: u64,
    },
    /// Projected successful function tool calls would exceed the configured budget.
    #[error("the next tool call(s) would exceed the tool_calls_limit of {limit} (tool_calls={tool_calls})")]
    ToolCalls {
        /// Configured tool-call limit.
        limit: u64,
        /// Projected successful tool calls.
        tool_calls: u64,
    },
}

const fn cost_for_tokens(tokens: u64, micros_per_million_tokens: u64) -> u64 {
    tokens
        .saturating_mul(micros_per_million_tokens)
        .saturating_add(999_999)
        / 1_000_000
}

const fn check_limit(
    kind: &'static str,
    limit: Option<u64>,
    actual: u64,
) -> Result<(), UsageLimitError> {
    if let Some(limit) = limit {
        if actual > limit {
            return Err(UsageLimitError::Token {
                kind,
                limit,
                actual,
            });
        }
    }
    Ok(())
}
