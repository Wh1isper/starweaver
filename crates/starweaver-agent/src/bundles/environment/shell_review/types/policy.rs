use serde::{Deserialize, Serialize};
use starweaver_model::{DynModelAdapter, ModelSettings};

/// Action applied when shell review reaches the configured approval threshold.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ShellReviewAction {
    /// Defer execution to the runtime HITL approval path.
    #[default]
    Defer,
    /// Block execution immediately and return a structured shell result.
    Deny,
}

/// Shell command review risk levels.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ShellReviewRiskLevel {
    /// Read-only inspection or local verification.
    #[default]
    Low,
    /// Bounded workspace-local state change.
    Medium,
    /// Broad destructive, privileged, external, or sensitive operation.
    High,
    /// Catastrophic or visibly hostile operation.
    ExtraHigh,
}

impl ShellReviewRiskLevel {
    /// Return a sortable risk rank where higher values require more caution.
    #[must_use]
    pub const fn rank(self) -> u8 {
        match self {
            Self::Low => 0,
            Self::Medium => 1,
            Self::High => 2,
            Self::ExtraHigh => 3,
        }
    }
}

/// Shell command safety review configuration.
#[derive(Clone)]
pub struct ShellReviewConfig {
    /// Whether review is enabled.
    pub enabled: bool,
    /// Model adapter used for review.
    pub model: Option<DynModelAdapter>,
    /// Optional model settings for review requests.
    pub model_settings: Option<ModelSettings>,
    /// Action when the risk threshold is reached.
    pub on_needs_approval: ShellReviewAction,
    /// Risk threshold requiring approval/deny handling.
    pub risk_threshold: ShellReviewRiskLevel,
    /// Optional override for the default review prompt.
    pub system_prompt: Option<String>,
}

impl ShellReviewConfig {
    /// Create an enabled shell review configuration using a model adapter.
    #[must_use]
    pub fn enabled(model: DynModelAdapter) -> Self {
        Self {
            enabled: true,
            model: Some(model),
            model_settings: None,
            on_needs_approval: ShellReviewAction::Defer,
            risk_threshold: ShellReviewRiskLevel::High,
            system_prompt: None,
        }
    }

    /// Create a disabled shell review configuration.
    #[must_use]
    pub const fn disabled() -> Self {
        Self {
            enabled: false,
            model: None,
            model_settings: None,
            on_needs_approval: ShellReviewAction::Defer,
            risk_threshold: ShellReviewRiskLevel::High,
            system_prompt: None,
        }
    }

    /// Attach model settings.
    #[must_use]
    pub fn with_model_settings(mut self, settings: ModelSettings) -> Self {
        self.model_settings = Some(settings);
        self
    }

    /// Set threshold action.
    #[must_use]
    pub const fn with_action(mut self, action: ShellReviewAction) -> Self {
        self.on_needs_approval = action;
        self
    }

    /// Set risk threshold.
    #[must_use]
    pub const fn with_risk_threshold(mut self, threshold: ShellReviewRiskLevel) -> Self {
        self.risk_threshold = threshold;
        self
    }

    /// Override the reviewer system prompt.
    #[must_use]
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }
}

impl Default for ShellReviewConfig {
    fn default() -> Self {
        Self::disabled()
    }
}

impl std::fmt::Debug for ShellReviewConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ShellReviewConfig")
            .field("enabled", &self.enabled)
            .field(
                "model",
                &self.model.as_ref().map(|model| model.model_name()),
            )
            .field("model_settings", &self.model_settings)
            .field("on_needs_approval", &self.on_needs_approval)
            .field("risk_threshold", &self.risk_threshold)
            .field(
                "system_prompt",
                &self.system_prompt.as_ref().map(|_| "<configured>"),
            )
            .finish()
    }
}

/// Structured shell review decision.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ShellReviewDecision {
    /// Review risk level.
    #[serde(default)]
    pub risk_level: ShellReviewRiskLevel,
    /// Concise reason for the decision.
    #[serde(default)]
    pub reason: String,
}

impl ShellReviewDecision {
    /// Return whether this decision reaches the configured threshold.
    #[must_use]
    pub const fn requires_approval(&self, config: &ShellReviewConfig) -> bool {
        config.enabled && self.risk_level.rank() >= config.risk_threshold.rank()
    }

    /// Return whether this decision should defer through HITL.
    #[must_use]
    pub fn requires_defer(&self, config: &ShellReviewConfig) -> bool {
        self.requires_approval(config) && config.on_needs_approval == ShellReviewAction::Defer
    }

    /// Return whether this decision should deny execution.
    #[must_use]
    pub fn requires_deny(&self, config: &ShellReviewConfig) -> bool {
        self.requires_approval(config) && config.on_needs_approval == ShellReviewAction::Deny
    }
}

pub(super) const fn risk_level_name(level: ShellReviewRiskLevel) -> &'static str {
    match level {
        ShellReviewRiskLevel::Low => "low",
        ShellReviewRiskLevel::Medium => "medium",
        ShellReviewRiskLevel::High => "high",
        ShellReviewRiskLevel::ExtraHigh => "extra_high",
    }
}
