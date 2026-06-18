//! Model, tool, and security configuration carried by agent context.

mod model;
mod tool;

use serde::{Deserialize, Serialize};

pub use model::{ModelCapability, ModelConfig};
pub use tool::{ToolAvailabilityPolicy, ToolConfig};

/// Fixed-point ratio stored as parts per thousand.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct PerThousandRatio {
    per_thousand: u16,
}

impl PerThousandRatio {
    /// Create a ratio from parts per thousand.
    #[must_use]
    pub const fn from_per_thousand(per_thousand: u16) -> Self {
        Self { per_thousand }
    }

    /// Return the ratio as parts per thousand.
    #[must_use]
    pub const fn per_thousand(self) -> u16 {
        self.per_thousand
    }

    /// Return the ratio as a floating point value for calculations.
    #[must_use]
    pub fn as_fraction(self) -> f64 {
        f64::from(self.per_thousand) / 1000.0
    }
}

impl Default for PerThousandRatio {
    fn default() -> Self {
        Self::from_per_thousand(1000)
    }
}

/// Shell review action for commands that require policy intervention.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ShellReviewAction {
    /// Defer command execution for external approval.
    #[default]
    Defer,
    /// Deny commands that need approval.
    Deny,
}

/// Shell review risk threshold.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ShellReviewRiskLevel {
    /// Low risk.
    Low,
    /// Medium risk.
    Medium,
    /// High risk.
    #[default]
    High,
    /// Extra high risk.
    ExtraHigh,
}

/// Shell command safety review configuration.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ShellReviewConfig {
    /// Whether shell review is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// Model identifier used for shell review.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Action when a command needs approval.
    #[serde(default)]
    pub on_needs_approval: ShellReviewAction,
    /// Risk level where intervention begins.
    #[serde(default)]
    pub risk_threshold: ShellReviewRiskLevel,
    /// Optional custom system prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
}

/// Security-related runtime configuration.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SecurityConfig {
    /// Optional shell command safety review configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell_review: Option<ShellReviewConfig>,
}

impl SecurityConfig {
    /// Return whether no security config is active.
    #[must_use]
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
}
