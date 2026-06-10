//! Built-in model presets for common provider configurations.

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use thiserror::Error;

use crate::{
    AuthConfig, HttpModelConfig, ModelProfile, ModelSettings, ProtocolFamily, ProviderAlias,
    ServiceTier, ThinkingSettings,
};

const K_TOKENS: u32 = 1024;
const ANTHROPIC_1M_BETA: &str = "context-1m-2025-08-07";
const ANTHROPIC_INTERLEAVED_BETA: &str = "interleaved-thinking-2025-05-14";
const ANTHROPIC_CONTEXT_MANAGEMENT_BETA: &str = "context-management-2025-06-27";

/// Preset lookup failure.
#[derive(Debug, Error)]
pub enum ModelPresetError {
    /// The requested preset name is unknown.
    #[error("unknown model preset: {name}. available: {available:?}")]
    UnknownPreset {
        /// Requested preset name.
        name: String,
        /// Available canonical names and aliases.
        available: Vec<String>,
    },
    /// The requested model config preset name is unknown.
    #[error("unknown model config preset: {name}. available: {available:?}")]
    UnknownModelConfig {
        /// Requested preset name.
        name: String,
        /// Available canonical names and aliases.
        available: Vec<String>,
    },
}

/// Built-in model settings preset names.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ModelSettingsPreset {
    /// `anthropic_default` model settings preset.
    #[serde(rename = "anthropic_default")]
    AnthropicDefault,
    /// `anthropic_high` model settings preset.
    #[serde(rename = "anthropic_high")]
    AnthropicHigh,
    /// `anthropic_medium` model settings preset.
    #[serde(rename = "anthropic_medium")]
    AnthropicMedium,
    /// `anthropic_low` model settings preset.
    #[serde(rename = "anthropic_low")]
    AnthropicLow,
    /// `anthropic_off` model settings preset.
    #[serde(rename = "anthropic_off")]
    AnthropicOff,
    /// `anthropic_adaptive_default` model settings preset.
    #[serde(rename = "anthropic_adaptive_default")]
    AnthropicAdaptiveDefault,
    /// `anthropic_adaptive_xhigh` model settings preset.
    #[serde(rename = "anthropic_adaptive_xhigh")]
    AnthropicAdaptiveXhigh,
    /// `anthropic_adaptive_high` model settings preset.
    #[serde(rename = "anthropic_adaptive_high")]
    AnthropicAdaptiveHigh,
    /// `anthropic_adaptive_medium` model settings preset.
    #[serde(rename = "anthropic_adaptive_medium")]
    AnthropicAdaptiveMedium,
    /// `anthropic_adaptive_low` model settings preset.
    #[serde(rename = "anthropic_adaptive_low")]
    AnthropicAdaptiveLow,
    /// `anthropic_adaptive_1m_default` model settings preset.
    #[serde(rename = "anthropic_adaptive_1m_default")]
    AnthropicAdaptive1mDefault,
    /// `anthropic_adaptive_1m_xhigh` model settings preset.
    #[serde(rename = "anthropic_adaptive_1m_xhigh")]
    AnthropicAdaptive1mXhigh,
    /// `anthropic_adaptive_1m_high` model settings preset.
    #[serde(rename = "anthropic_adaptive_1m_high")]
    AnthropicAdaptive1mHigh,
    /// `anthropic_adaptive_1m_medium` model settings preset.
    #[serde(rename = "anthropic_adaptive_1m_medium")]
    AnthropicAdaptive1mMedium,
    /// `anthropic_adaptive_1m_low` model settings preset.
    #[serde(rename = "anthropic_adaptive_1m_low")]
    AnthropicAdaptive1mLow,
    /// `anthropic_adaptive_cm_default` model settings preset.
    #[serde(rename = "anthropic_adaptive_cm_default")]
    AnthropicAdaptiveCmDefault,
    /// `anthropic_adaptive_cm_xhigh` model settings preset.
    #[serde(rename = "anthropic_adaptive_cm_xhigh")]
    AnthropicAdaptiveCmXhigh,
    /// `anthropic_adaptive_cm_high` model settings preset.
    #[serde(rename = "anthropic_adaptive_cm_high")]
    AnthropicAdaptiveCmHigh,
    /// `anthropic_adaptive_cm_medium` model settings preset.
    #[serde(rename = "anthropic_adaptive_cm_medium")]
    AnthropicAdaptiveCmMedium,
    /// `anthropic_adaptive_cm_low` model settings preset.
    #[serde(rename = "anthropic_adaptive_cm_low")]
    AnthropicAdaptiveCmLow,
    /// `anthropic_adaptive_1m_cm_default` model settings preset.
    #[serde(rename = "anthropic_adaptive_1m_cm_default")]
    AnthropicAdaptive1mCmDefault,
    /// `anthropic_adaptive_1m_cm_xhigh` model settings preset.
    #[serde(rename = "anthropic_adaptive_1m_cm_xhigh")]
    AnthropicAdaptive1mCmXhigh,
    /// `anthropic_adaptive_1m_cm_high` model settings preset.
    #[serde(rename = "anthropic_adaptive_1m_cm_high")]
    AnthropicAdaptive1mCmHigh,
    /// `anthropic_adaptive_1m_cm_medium` model settings preset.
    #[serde(rename = "anthropic_adaptive_1m_cm_medium")]
    AnthropicAdaptive1mCmMedium,
    /// `anthropic_adaptive_1m_cm_low` model settings preset.
    #[serde(rename = "anthropic_adaptive_1m_cm_low")]
    AnthropicAdaptive1mCmLow,
    /// `anthropic_default_interleaved_thinking` model settings preset.
    #[serde(rename = "anthropic_default_interleaved_thinking")]
    AnthropicDefaultInterleavedThinking,
    /// `anthropic_high_interleaved_thinking` model settings preset.
    #[serde(rename = "anthropic_high_interleaved_thinking")]
    AnthropicHighInterleavedThinking,
    /// `anthropic_medium_interleaved_thinking` model settings preset.
    #[serde(rename = "anthropic_medium_interleaved_thinking")]
    AnthropicMediumInterleavedThinking,
    /// `anthropic_low_interleaved_thinking` model settings preset.
    #[serde(rename = "anthropic_low_interleaved_thinking")]
    AnthropicLowInterleavedThinking,
    /// `anthropic_off_interleaved_thinking` model settings preset.
    #[serde(rename = "anthropic_off_interleaved_thinking")]
    AnthropicOffInterleavedThinking,
    /// `anthropic_1m_default` model settings preset.
    #[serde(rename = "anthropic_1m_default")]
    Anthropic1mDefault,
    /// `anthropic_1m_high` model settings preset.
    #[serde(rename = "anthropic_1m_high")]
    Anthropic1mHigh,
    /// `anthropic_1m_medium` model settings preset.
    #[serde(rename = "anthropic_1m_medium")]
    Anthropic1mMedium,
    /// `anthropic_1m_low` model settings preset.
    #[serde(rename = "anthropic_1m_low")]
    Anthropic1mLow,
    /// `anthropic_1m_off` model settings preset.
    #[serde(rename = "anthropic_1m_off")]
    Anthropic1mOff,
    /// `anthropic_1m_default_interleaved_thinking` model settings preset.
    #[serde(rename = "anthropic_1m_default_interleaved_thinking")]
    Anthropic1mDefaultInterleavedThinking,
    /// `anthropic_1m_high_interleaved_thinking` model settings preset.
    #[serde(rename = "anthropic_1m_high_interleaved_thinking")]
    Anthropic1mHighInterleavedThinking,
    /// `anthropic_1m_medium_interleaved_thinking` model settings preset.
    #[serde(rename = "anthropic_1m_medium_interleaved_thinking")]
    Anthropic1mMediumInterleavedThinking,
    /// `anthropic_1m_low_interleaved_thinking` model settings preset.
    #[serde(rename = "anthropic_1m_low_interleaved_thinking")]
    Anthropic1mLowInterleavedThinking,
    /// `anthropic_1m_off_interleaved_thinking` model settings preset.
    #[serde(rename = "anthropic_1m_off_interleaved_thinking")]
    Anthropic1mOffInterleavedThinking,
    /// `anthropic_cm_default` model settings preset.
    #[serde(rename = "anthropic_cm_default")]
    AnthropicCmDefault,
    /// `anthropic_cm_high` model settings preset.
    #[serde(rename = "anthropic_cm_high")]
    AnthropicCmHigh,
    /// `anthropic_cm_medium` model settings preset.
    #[serde(rename = "anthropic_cm_medium")]
    AnthropicCmMedium,
    /// `anthropic_cm_low` model settings preset.
    #[serde(rename = "anthropic_cm_low")]
    AnthropicCmLow,
    /// `anthropic_cm_off` model settings preset.
    #[serde(rename = "anthropic_cm_off")]
    AnthropicCmOff,
    /// `anthropic_1m_cm_default` model settings preset.
    #[serde(rename = "anthropic_1m_cm_default")]
    Anthropic1mCmDefault,
    /// `anthropic_1m_cm_high` model settings preset.
    #[serde(rename = "anthropic_1m_cm_high")]
    Anthropic1mCmHigh,
    /// `anthropic_1m_cm_medium` model settings preset.
    #[serde(rename = "anthropic_1m_cm_medium")]
    Anthropic1mCmMedium,
    /// `anthropic_1m_cm_low` model settings preset.
    #[serde(rename = "anthropic_1m_cm_low")]
    Anthropic1mCmLow,
    /// `anthropic_1m_cm_off` model settings preset.
    #[serde(rename = "anthropic_1m_cm_off")]
    Anthropic1mCmOff,
    /// `anthropic_cm_default_interleaved_thinking` model settings preset.
    #[serde(rename = "anthropic_cm_default_interleaved_thinking")]
    AnthropicCmDefaultInterleavedThinking,
    /// `anthropic_cm_high_interleaved_thinking` model settings preset.
    #[serde(rename = "anthropic_cm_high_interleaved_thinking")]
    AnthropicCmHighInterleavedThinking,
    /// `anthropic_cm_medium_interleaved_thinking` model settings preset.
    #[serde(rename = "anthropic_cm_medium_interleaved_thinking")]
    AnthropicCmMediumInterleavedThinking,
    /// `anthropic_cm_low_interleaved_thinking` model settings preset.
    #[serde(rename = "anthropic_cm_low_interleaved_thinking")]
    AnthropicCmLowInterleavedThinking,
    /// `anthropic_cm_off_interleaved_thinking` model settings preset.
    #[serde(rename = "anthropic_cm_off_interleaved_thinking")]
    AnthropicCmOffInterleavedThinking,
    /// `anthropic_1m_cm_default_interleaved_thinking` model settings preset.
    #[serde(rename = "anthropic_1m_cm_default_interleaved_thinking")]
    Anthropic1mCmDefaultInterleavedThinking,
    /// `anthropic_1m_cm_high_interleaved_thinking` model settings preset.
    #[serde(rename = "anthropic_1m_cm_high_interleaved_thinking")]
    Anthropic1mCmHighInterleavedThinking,
    /// `anthropic_1m_cm_medium_interleaved_thinking` model settings preset.
    #[serde(rename = "anthropic_1m_cm_medium_interleaved_thinking")]
    Anthropic1mCmMediumInterleavedThinking,
    /// `anthropic_1m_cm_low_interleaved_thinking` model settings preset.
    #[serde(rename = "anthropic_1m_cm_low_interleaved_thinking")]
    Anthropic1mCmLowInterleavedThinking,
    /// `anthropic_1m_cm_off_interleaved_thinking` model settings preset.
    #[serde(rename = "anthropic_1m_cm_off_interleaved_thinking")]
    Anthropic1mCmOffInterleavedThinking,
    /// `openai_default` model settings preset.
    #[serde(rename = "openai_default")]
    OpenAiDefault,
    /// `openai_xhigh` model settings preset.
    #[serde(rename = "openai_xhigh")]
    OpenAiXhigh,
    /// `openai_high` model settings preset.
    #[serde(rename = "openai_high")]
    OpenAiHigh,
    /// `openai_medium` model settings preset.
    #[serde(rename = "openai_medium")]
    OpenAiMedium,
    /// `openai_low` model settings preset.
    #[serde(rename = "openai_low")]
    OpenAiLow,
    /// `openai_responses_default` model settings preset.
    #[serde(rename = "openai_responses_default")]
    OpenAiResponsesDefault,
    /// `openai_responses_xhigh` model settings preset.
    #[serde(rename = "openai_responses_xhigh")]
    OpenAiResponsesXhigh,
    /// `openai_responses_high` model settings preset.
    #[serde(rename = "openai_responses_high")]
    OpenAiResponsesHigh,
    /// `openai_responses_medium` model settings preset.
    #[serde(rename = "openai_responses_medium")]
    OpenAiResponsesMedium,
    /// `openai_responses_low` model settings preset.
    #[serde(rename = "openai_responses_low")]
    OpenAiResponsesLow,
    /// `openai_responses_default_fast` model settings preset.
    #[serde(rename = "openai_responses_default_fast")]
    OpenAiResponsesDefaultFast,
    /// `openai_responses_xhigh_fast` model settings preset.
    #[serde(rename = "openai_responses_xhigh_fast")]
    OpenAiResponsesXhighFast,
    /// `openai_responses_high_fast` model settings preset.
    #[serde(rename = "openai_responses_high_fast")]
    OpenAiResponsesHighFast,
    /// `openai_responses_medium_fast` model settings preset.
    #[serde(rename = "openai_responses_medium_fast")]
    OpenAiResponsesMediumFast,
    /// `openai_responses_low_fast` model settings preset.
    #[serde(rename = "openai_responses_low_fast")]
    OpenAiResponsesLowFast,
    /// `deepseek_v4_default` model settings preset.
    #[serde(rename = "deepseek_v4_default")]
    DeepSeekV4Default,
    /// `deepseek_v4_high` model settings preset.
    #[serde(rename = "deepseek_v4_high")]
    DeepSeekV4High,
    /// `deepseek_v4_max` model settings preset.
    #[serde(rename = "deepseek_v4_max")]
    DeepSeekV4Max,
    /// `deepseek_v4_off` model settings preset.
    #[serde(rename = "deepseek_v4_off")]
    DeepSeekV4Off,
    /// `mimo_v2_5` model settings preset.
    #[serde(rename = "mimo_v2_5")]
    MimoV25,
    /// `mimo_v2_5_pro` model settings preset.
    #[serde(rename = "mimo_v2_5_pro")]
    MimoV25Pro,
    /// `gemini_thinking_budget_default` model settings preset.
    #[serde(rename = "gemini_thinking_budget_default")]
    GeminiThinkingBudgetDefault,
    /// `gemini_thinking_budget_high` model settings preset.
    #[serde(rename = "gemini_thinking_budget_high")]
    GeminiThinkingBudgetHigh,
    /// `gemini_thinking_budget_medium` model settings preset.
    #[serde(rename = "gemini_thinking_budget_medium")]
    GeminiThinkingBudgetMedium,
    /// `gemini_thinking_budget_low` model settings preset.
    #[serde(rename = "gemini_thinking_budget_low")]
    GeminiThinkingBudgetLow,
    /// `gemini_thinking_level_default` model settings preset.
    #[serde(rename = "gemini_thinking_level_default")]
    GeminiThinkingLevelDefault,
    /// `gemini_thinking_level_high` model settings preset.
    #[serde(rename = "gemini_thinking_level_high")]
    GeminiThinkingLevelHigh,
    /// `gemini_thinking_level_medium` model settings preset.
    #[serde(rename = "gemini_thinking_level_medium")]
    GeminiThinkingLevelMedium,
    /// `gemini_thinking_level_low` model settings preset.
    #[serde(rename = "gemini_thinking_level_low")]
    GeminiThinkingLevelLow,
    /// `gemini_thinking_level_minimal` model settings preset.
    #[serde(rename = "gemini_thinking_level_minimal")]
    GeminiThinkingLevelMinimal,
}

impl ModelSettingsPreset {
    /// Return the canonical preset name.
    #[must_use]
    #[allow(clippy::too_many_lines)]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AnthropicDefault => "anthropic_default",
            Self::AnthropicHigh => "anthropic_high",
            Self::AnthropicMedium => "anthropic_medium",
            Self::AnthropicLow => "anthropic_low",
            Self::AnthropicOff => "anthropic_off",
            Self::AnthropicAdaptiveDefault => "anthropic_adaptive_default",
            Self::AnthropicAdaptiveXhigh => "anthropic_adaptive_xhigh",
            Self::AnthropicAdaptiveHigh => "anthropic_adaptive_high",
            Self::AnthropicAdaptiveMedium => "anthropic_adaptive_medium",
            Self::AnthropicAdaptiveLow => "anthropic_adaptive_low",
            Self::AnthropicAdaptive1mDefault => "anthropic_adaptive_1m_default",
            Self::AnthropicAdaptive1mXhigh => "anthropic_adaptive_1m_xhigh",
            Self::AnthropicAdaptive1mHigh => "anthropic_adaptive_1m_high",
            Self::AnthropicAdaptive1mMedium => "anthropic_adaptive_1m_medium",
            Self::AnthropicAdaptive1mLow => "anthropic_adaptive_1m_low",
            Self::AnthropicAdaptiveCmDefault => "anthropic_adaptive_cm_default",
            Self::AnthropicAdaptiveCmXhigh => "anthropic_adaptive_cm_xhigh",
            Self::AnthropicAdaptiveCmHigh => "anthropic_adaptive_cm_high",
            Self::AnthropicAdaptiveCmMedium => "anthropic_adaptive_cm_medium",
            Self::AnthropicAdaptiveCmLow => "anthropic_adaptive_cm_low",
            Self::AnthropicAdaptive1mCmDefault => "anthropic_adaptive_1m_cm_default",
            Self::AnthropicAdaptive1mCmXhigh => "anthropic_adaptive_1m_cm_xhigh",
            Self::AnthropicAdaptive1mCmHigh => "anthropic_adaptive_1m_cm_high",
            Self::AnthropicAdaptive1mCmMedium => "anthropic_adaptive_1m_cm_medium",
            Self::AnthropicAdaptive1mCmLow => "anthropic_adaptive_1m_cm_low",
            Self::AnthropicDefaultInterleavedThinking => "anthropic_default_interleaved_thinking",
            Self::AnthropicHighInterleavedThinking => "anthropic_high_interleaved_thinking",
            Self::AnthropicMediumInterleavedThinking => "anthropic_medium_interleaved_thinking",
            Self::AnthropicLowInterleavedThinking => "anthropic_low_interleaved_thinking",
            Self::AnthropicOffInterleavedThinking => "anthropic_off_interleaved_thinking",
            Self::Anthropic1mDefault => "anthropic_1m_default",
            Self::Anthropic1mHigh => "anthropic_1m_high",
            Self::Anthropic1mMedium => "anthropic_1m_medium",
            Self::Anthropic1mLow => "anthropic_1m_low",
            Self::Anthropic1mOff => "anthropic_1m_off",
            Self::Anthropic1mDefaultInterleavedThinking => {
                "anthropic_1m_default_interleaved_thinking"
            }
            Self::Anthropic1mHighInterleavedThinking => "anthropic_1m_high_interleaved_thinking",
            Self::Anthropic1mMediumInterleavedThinking => {
                "anthropic_1m_medium_interleaved_thinking"
            }
            Self::Anthropic1mLowInterleavedThinking => "anthropic_1m_low_interleaved_thinking",
            Self::Anthropic1mOffInterleavedThinking => "anthropic_1m_off_interleaved_thinking",
            Self::AnthropicCmDefault => "anthropic_cm_default",
            Self::AnthropicCmHigh => "anthropic_cm_high",
            Self::AnthropicCmMedium => "anthropic_cm_medium",
            Self::AnthropicCmLow => "anthropic_cm_low",
            Self::AnthropicCmOff => "anthropic_cm_off",
            Self::Anthropic1mCmDefault => "anthropic_1m_cm_default",
            Self::Anthropic1mCmHigh => "anthropic_1m_cm_high",
            Self::Anthropic1mCmMedium => "anthropic_1m_cm_medium",
            Self::Anthropic1mCmLow => "anthropic_1m_cm_low",
            Self::Anthropic1mCmOff => "anthropic_1m_cm_off",
            Self::AnthropicCmDefaultInterleavedThinking => {
                "anthropic_cm_default_interleaved_thinking"
            }
            Self::AnthropicCmHighInterleavedThinking => "anthropic_cm_high_interleaved_thinking",
            Self::AnthropicCmMediumInterleavedThinking => {
                "anthropic_cm_medium_interleaved_thinking"
            }
            Self::AnthropicCmLowInterleavedThinking => "anthropic_cm_low_interleaved_thinking",
            Self::AnthropicCmOffInterleavedThinking => "anthropic_cm_off_interleaved_thinking",
            Self::Anthropic1mCmDefaultInterleavedThinking => {
                "anthropic_1m_cm_default_interleaved_thinking"
            }
            Self::Anthropic1mCmHighInterleavedThinking => {
                "anthropic_1m_cm_high_interleaved_thinking"
            }
            Self::Anthropic1mCmMediumInterleavedThinking => {
                "anthropic_1m_cm_medium_interleaved_thinking"
            }
            Self::Anthropic1mCmLowInterleavedThinking => "anthropic_1m_cm_low_interleaved_thinking",
            Self::Anthropic1mCmOffInterleavedThinking => "anthropic_1m_cm_off_interleaved_thinking",
            Self::OpenAiDefault => "openai_default",
            Self::OpenAiXhigh => "openai_xhigh",
            Self::OpenAiHigh => "openai_high",
            Self::OpenAiMedium => "openai_medium",
            Self::OpenAiLow => "openai_low",
            Self::OpenAiResponsesDefault => "openai_responses_default",
            Self::OpenAiResponsesXhigh => "openai_responses_xhigh",
            Self::OpenAiResponsesHigh => "openai_responses_high",
            Self::OpenAiResponsesMedium => "openai_responses_medium",
            Self::OpenAiResponsesLow => "openai_responses_low",
            Self::OpenAiResponsesDefaultFast => "openai_responses_default_fast",
            Self::OpenAiResponsesXhighFast => "openai_responses_xhigh_fast",
            Self::OpenAiResponsesHighFast => "openai_responses_high_fast",
            Self::OpenAiResponsesMediumFast => "openai_responses_medium_fast",
            Self::OpenAiResponsesLowFast => "openai_responses_low_fast",
            Self::DeepSeekV4Default => "deepseek_v4_default",
            Self::DeepSeekV4High => "deepseek_v4_high",
            Self::DeepSeekV4Max => "deepseek_v4_max",
            Self::DeepSeekV4Off => "deepseek_v4_off",
            Self::MimoV25 => "mimo_v2_5",
            Self::MimoV25Pro => "mimo_v2_5_pro",
            Self::GeminiThinkingBudgetDefault => "gemini_thinking_budget_default",
            Self::GeminiThinkingBudgetHigh => "gemini_thinking_budget_high",
            Self::GeminiThinkingBudgetMedium => "gemini_thinking_budget_medium",
            Self::GeminiThinkingBudgetLow => "gemini_thinking_budget_low",
            Self::GeminiThinkingLevelDefault => "gemini_thinking_level_default",
            Self::GeminiThinkingLevelHigh => "gemini_thinking_level_high",
            Self::GeminiThinkingLevelMedium => "gemini_thinking_level_medium",
            Self::GeminiThinkingLevelLow => "gemini_thinking_level_low",
            Self::GeminiThinkingLevelMinimal => "gemini_thinking_level_minimal",
        }
    }
}

/// Built-in model capability/config preset names.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ModelConfigPreset {
    /// Claude 200K context profile.
    #[serde(rename = "claude_200k")]
    Claude200k,
    /// Claude 400K context profile.
    #[serde(rename = "claude_400k")]
    Claude400k,
    /// Claude 1M context profile.
    #[serde(rename = "claude_1m")]
    Claude1m,
    /// GPT-5 270K context profile.
    #[serde(rename = "gpt5_270k")]
    Gpt5_270k,
    /// GPT-5 1M class context profile.
    #[serde(rename = "gpt5_1m")]
    Gpt5_1m,
    /// `DeepSeek` V4 400K context profile.
    #[serde(rename = "deepseek_v4_400k")]
    DeepSeekV4_400k,
    /// `DeepSeek` V4 1M context profile.
    #[serde(rename = "deepseek_v4_1m")]
    DeepSeekV4_1m,
    /// `MiMo` V2.5 1M context profile.
    #[serde(rename = "mimo_v2_5_1m")]
    MimoV25_1m,
    /// `MiMo` V2.5 Pro 1M context profile.
    #[serde(rename = "mimo_v2_5_pro_1m")]
    MimoV25Pro1m,
    /// Gemini 200K context profile.
    #[serde(rename = "gemini_200k")]
    Gemini200k,
    /// Gemini 1M context profile.
    #[serde(rename = "gemini_1m")]
    Gemini1m,
}

impl ModelConfigPreset {
    /// Return the canonical preset name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Claude200k => "claude_200k",
            Self::Claude400k => "claude_400k",
            Self::Claude1m => "claude_1m",
            Self::Gpt5_270k => "gpt5_270k",
            Self::Gpt5_1m => "gpt5_1m",
            Self::DeepSeekV4_400k => "deepseek_v4_400k",
            Self::DeepSeekV4_1m => "deepseek_v4_1m",
            Self::MimoV25_1m => "mimo_v2_5_1m",
            Self::MimoV25Pro1m => "mimo_v2_5_pro_1m",
            Self::Gemini200k => "gemini_200k",
            Self::Gemini1m => "gemini_1m",
        }
    }
}

/// Media and context metadata for a configured model family.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ModelConfigPresetData {
    /// Canonical preset name.
    pub name: String,
    /// Provider protocol family.
    pub protocol: ProtocolFamily,
    /// Context window in tokens.
    pub context_window: u32,
    /// Maximum image count recommended for one request.
    pub max_images: u32,
    /// Maximum video count recommended for one request.
    pub max_videos: u32,
    /// GIF input support.
    pub supports_gif: bool,
    /// Split large images before sending them to the model.
    pub split_large_images: bool,
    /// Maximum image split height.
    pub image_split_max_height: u32,
    /// Image split overlap.
    pub image_split_overlap: u32,
    /// Model profile capabilities.
    pub profile: ModelProfile,
}

/// Complete model preset ready for host profile resolution.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ModelRuntimePreset {
    /// Canonical preset name.
    pub name: String,
    /// Provider alias/model id.
    pub model_id: String,
    /// Provider display name.
    pub provider_name: String,
    /// Provider model name sent on the wire.
    pub model_name: String,
    /// Protocol family.
    pub protocol: ProtocolFamily,
    /// Default model settings.
    pub settings: ModelSettings,
    /// Capability/config preset.
    pub config: ModelConfigPresetData,
}

impl ModelRuntimePreset {
    /// Convert this runtime preset into a provider alias using the supplied HTTP config.
    #[must_use]
    pub fn provider_alias(&self, http: HttpModelConfig) -> ProviderAlias {
        ProviderAlias::new(
            self.model_id.clone(),
            self.provider_name.clone(),
            self.model_name.clone(),
            self.protocol,
            http,
        )
        .with_profile(self.config.profile.clone())
        .with_default_settings(self.settings.clone())
    }
}

/// Resolve a built-in model settings preset by name or alias.
///
/// # Errors
///
/// Returns an error when the preset name is unknown.
pub fn get_model_settings(name: &str) -> Result<ModelSettings, ModelPresetError> {
    let canonical = model_settings_alias(name);
    model_settings_by_name(canonical).ok_or_else(|| ModelPresetError::UnknownPreset {
        name: name.to_string(),
        available: list_model_settings_presets(),
    })
}

/// Return a built-in model config preset by name or alias.
///
/// # Errors
///
/// Returns an error when the preset name is unknown.
pub fn get_model_config(name: &str) -> Result<ModelConfigPresetData, ModelPresetError> {
    let canonical = model_config_alias(name);
    model_config_by_name(canonical).ok_or_else(|| ModelPresetError::UnknownModelConfig {
        name: name.to_string(),
        available: list_model_config_presets(),
    })
}

/// Return all built-in model settings preset names and aliases.
#[must_use]
pub fn list_model_settings_presets() -> Vec<String> {
    let mut names = MODEL_SETTINGS_PRESETS
        .iter()
        .copied()
        .chain(MODEL_SETTINGS_ALIASES.iter().map(|(alias, _)| *alias))
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    names.sort();
    names.dedup();
    names
}

/// Return all built-in model config preset names and aliases.
#[must_use]
pub fn list_model_config_presets() -> Vec<String> {
    let mut names = MODEL_CONFIG_PRESETS
        .iter()
        .copied()
        .chain(MODEL_CONFIG_ALIASES.iter().map(|(alias, _)| *alias))
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    names.sort();
    names.dedup();
    names
}

/// Build a complete runtime preset from model id, settings preset, and config preset.
///
/// # Errors
///
/// Returns an error when a preset name is unknown.
pub fn model_runtime_preset(
    model_id: impl Into<String>,
    provider_name: impl Into<String>,
    model_name: impl Into<String>,
    settings_preset: &str,
    config_preset: &str,
) -> Result<ModelRuntimePreset, ModelPresetError> {
    let settings = get_model_settings(settings_preset)?;
    let config = get_model_config(config_preset)?;
    Ok(ModelRuntimePreset {
        name: format!("{}+{}", model_settings_alias(settings_preset), config.name),
        model_id: model_id.into(),
        provider_name: provider_name.into(),
        model_name: model_name.into(),
        protocol: config.protocol,
        settings,
        config,
    })
}

/// Build an Anthropic HTTP model config from an API key.
#[must_use]
pub fn anthropic_http_config(api_key: impl Into<String>) -> HttpModelConfig {
    let mut config = HttpModelConfig::new("https://api.anthropic.com/v1", "messages");
    config.auth = Some(AuthConfig::Header {
        name: "x-api-key".to_string(),
        value: api_key.into(),
    });
    config
        .headers
        .insert("anthropic-version".to_string(), "2023-06-01".to_string());
    config
}

/// Build an `OpenAI` Chat Completions HTTP model config from an API key.
#[must_use]
pub fn openai_chat_http_config(api_key: impl Into<String>) -> HttpModelConfig {
    let mut config = HttpModelConfig::new("https://api.openai.com/v1", "chat/completions");
    config.auth = Some(AuthConfig::Bearer {
        token: api_key.into(),
    });
    config
}

/// Build an `OpenAI` Responses HTTP model config from an API key.
#[must_use]
pub fn openai_responses_http_config(api_key: impl Into<String>) -> HttpModelConfig {
    let mut config = HttpModelConfig::new("https://api.openai.com/v1", "responses");
    config.auth = Some(AuthConfig::Bearer {
        token: api_key.into(),
    });
    config
}

/// Build a Gemini HTTP model config from an API key and model name.
#[must_use]
pub fn gemini_http_config(
    api_key: impl Into<String>,
    model_name: impl Into<String>,
) -> HttpModelConfig {
    let model_name = model_name.into();
    HttpModelConfig::new(
        "https://generativelanguage.googleapis.com/v1beta",
        format!("models/{model_name}:generateContent?key={}", api_key.into()),
    )
}

#[allow(clippy::match_same_arms)]
fn model_settings_by_name(name: &str) -> Option<ModelSettings> {
    if let Some(spec) = parse_anthropic_preset(name)
        .or_else(|| parse_anthropic_preset(anthropic_legacy_alias(name)))
    {
        return Some(match spec.kind {
            AnthropicPresetKind::Adaptive { effort, max_tokens } => anthropic_adaptive(
                effort,
                max_tokens,
                spec.use_1m,
                spec.use_interleaved,
                spec.use_context_management,
            ),
            AnthropicPresetKind::Off => anthropic_off(
                spec.use_1m,
                spec.use_interleaved,
                spec.use_context_management,
            ),
        });
    }
    match name {
        "openai_default" => Some(openai_chat("medium", 8 * K_TOKENS)),
        "openai_xhigh" => Some(openai_chat("xhigh", 32 * K_TOKENS)),
        "openai_high" => Some(openai_chat("high", 16 * K_TOKENS)),
        "openai_medium" => Some(openai_chat("medium", 8 * K_TOKENS)),
        "openai_low" => Some(openai_chat("low", 4 * K_TOKENS)),
        "openai_responses_default" => Some(openai_responses("medium", "auto", 16 * K_TOKENS, None)),
        "openai_responses_xhigh" => {
            Some(openai_responses("xhigh", "detailed", 64 * K_TOKENS, None))
        }
        "openai_responses_high" => Some(openai_responses("high", "detailed", 32 * K_TOKENS, None)),
        "openai_responses_medium" => Some(openai_responses("medium", "auto", 16 * K_TOKENS, None)),
        "openai_responses_low" => Some(openai_responses("low", "concise", 8 * K_TOKENS, None)),
        "openai_responses_default_fast" => Some(openai_responses(
            "medium",
            "auto",
            16 * K_TOKENS,
            Some(ServiceTier::Priority),
        )),
        "openai_responses_xhigh_fast" => Some(openai_responses(
            "xhigh",
            "detailed",
            64 * K_TOKENS,
            Some(ServiceTier::Priority),
        )),
        "openai_responses_high_fast" => Some(openai_responses(
            "high",
            "detailed",
            32 * K_TOKENS,
            Some(ServiceTier::Priority),
        )),
        "openai_responses_medium_fast" => Some(openai_responses(
            "medium",
            "auto",
            16 * K_TOKENS,
            Some(ServiceTier::Priority),
        )),
        "openai_responses_low_fast" => Some(openai_responses(
            "low",
            "concise",
            8 * K_TOKENS,
            Some(ServiceTier::Priority),
        )),
        "deepseek_v4_default" | "deepseek_v4_high" => Some(openai_compatible_thinking(
            "high",
            Some(128 * K_TOKENS),
            true,
        )),
        "deepseek_v4_max" => Some(openai_compatible_thinking(
            "max",
            Some(384 * K_TOKENS),
            true,
        )),
        "deepseek_v4_off" => Some(openai_compatible_thinking(
            "high",
            Some(128 * K_TOKENS),
            false,
        )),
        "mimo_v2_5" | "mimo_v2_5_pro" => Some(mimo_v2_5()),
        "gemini_thinking_budget_default" | "gemini_thinking_budget_medium" => {
            Some(gemini_budget(16 * K_TOKENS, 16 * K_TOKENS))
        }
        "gemini_thinking_budget_high" => Some(gemini_budget(32 * K_TOKENS, 21 * K_TOKENS)),
        "gemini_thinking_budget_low" => Some(gemini_budget(4 * K_TOKENS, 8 * K_TOKENS)),
        "gemini_thinking_level_default" => Some(gemini_level("LOW", 16 * K_TOKENS)),
        "gemini_thinking_level_low" => Some(gemini_level("LOW", 8 * K_TOKENS)),
        "gemini_thinking_level_high" => Some(gemini_level("HIGH", 21 * K_TOKENS)),
        "gemini_thinking_level_medium" => Some(gemini_level("MEDIUM", 16 * K_TOKENS)),
        "gemini_thinking_level_minimal" => Some(gemini_level("MINIMAL", 4 * K_TOKENS)),
        _ => None,
    }
}

#[derive(Clone, Copy)]
struct AnthropicPresetSpec {
    kind: AnthropicPresetKind,
    use_1m: bool,
    use_interleaved: bool,
    use_context_management: bool,
}

#[derive(Clone, Copy)]
enum AnthropicPresetKind {
    Adaptive {
        effort: &'static str,
        max_tokens: u32,
    },
    Off,
}

fn anthropic_legacy_alias(name: &str) -> &str {
    match name {
        "anthropic_default" | "anthropic_default_interleaved_thinking" => {
            "anthropic_adaptive_default"
        }
        "anthropic_high" | "anthropic_high_interleaved_thinking" => "anthropic_adaptive_high",
        "anthropic_medium" | "anthropic_medium_interleaved_thinking" => "anthropic_adaptive_medium",
        "anthropic_low" | "anthropic_low_interleaved_thinking" => "anthropic_adaptive_low",
        "anthropic_off_interleaved_thinking" => "anthropic_off",
        "anthropic_1m_default" | "anthropic_1m_default_interleaved_thinking" => {
            "anthropic_adaptive_1m_default"
        }
        "anthropic_1m_high" | "anthropic_1m_high_interleaved_thinking" => {
            "anthropic_adaptive_1m_high"
        }
        "anthropic_1m_medium" | "anthropic_1m_medium_interleaved_thinking" => {
            "anthropic_adaptive_1m_medium"
        }
        "anthropic_1m_low" | "anthropic_1m_low_interleaved_thinking" => "anthropic_adaptive_1m_low",
        "anthropic_1m_off_interleaved_thinking" => "anthropic_1m_off",
        "anthropic_cm_default" | "anthropic_cm_default_interleaved_thinking" => {
            "anthropic_adaptive_cm_default"
        }
        "anthropic_cm_high" | "anthropic_cm_high_interleaved_thinking" => {
            "anthropic_adaptive_cm_high"
        }
        "anthropic_cm_medium" | "anthropic_cm_medium_interleaved_thinking" => {
            "anthropic_adaptive_cm_medium"
        }
        "anthropic_cm_low" | "anthropic_cm_low_interleaved_thinking" => "anthropic_adaptive_cm_low",
        "anthropic_cm_off_interleaved_thinking" => "anthropic_cm_off",
        "anthropic_1m_cm_default" | "anthropic_1m_cm_default_interleaved_thinking" => {
            "anthropic_adaptive_1m_cm_default"
        }
        "anthropic_1m_cm_high" | "anthropic_1m_cm_high_interleaved_thinking" => {
            "anthropic_adaptive_1m_cm_high"
        }
        "anthropic_1m_cm_medium" | "anthropic_1m_cm_medium_interleaved_thinking" => {
            "anthropic_adaptive_1m_cm_medium"
        }
        "anthropic_1m_cm_low" | "anthropic_1m_cm_low_interleaved_thinking" => {
            "anthropic_adaptive_1m_cm_low"
        }
        "anthropic_1m_cm_off_interleaved_thinking" => "anthropic_1m_cm_off",
        other => other,
    }
}

fn parse_anthropic_preset(name: &str) -> Option<AnthropicPresetSpec> {
    let (name, use_interleaved) = name
        .strip_suffix("_interleaved_thinking")
        .map_or((name, false), |name| (name, true));
    let without_prefix = name.strip_prefix("anthropic_")?;
    let (use_1m, rest) = without_prefix.strip_prefix("adaptive_1m_").map_or_else(
        || {
            without_prefix
                .strip_prefix("1m_")
                .map_or((false, without_prefix), |rest| (true, rest))
        },
        |rest| (true, rest),
    );
    let rest = rest.strip_prefix("adaptive_").unwrap_or(rest);
    let (use_context_management, rest) = rest
        .strip_prefix("cm_")
        .map_or((false, rest), |rest| (true, rest));
    let kind = match rest {
        "default" | "high" => AnthropicPresetKind::Adaptive {
            effort: "high",
            max_tokens: 32 * K_TOKENS,
        },
        "xhigh" => AnthropicPresetKind::Adaptive {
            effort: "xhigh",
            max_tokens: 64 * K_TOKENS,
        },
        "medium" => AnthropicPresetKind::Adaptive {
            effort: "medium",
            max_tokens: 21 * K_TOKENS,
        },
        "low" => AnthropicPresetKind::Adaptive {
            effort: "low",
            max_tokens: 16 * K_TOKENS,
        },
        "off" => AnthropicPresetKind::Off,
        _ => return None,
    };
    Some(AnthropicPresetSpec {
        kind,
        use_1m,
        use_interleaved,
        use_context_management,
    })
}

#[allow(clippy::match_same_arms, clippy::too_many_lines)]
fn model_config_by_name(name: &str) -> Option<ModelConfigPresetData> {
    match name {
        "claude_200k" => Some(config_data(
            name,
            ProtocolFamily::AnthropicMessages,
            200_000,
            20,
            0,
            true,
            true,
        )),
        "claude_400k" => Some(config_data(
            name,
            ProtocolFamily::AnthropicMessages,
            400_000,
            20,
            0,
            true,
            true,
        )),
        "claude_1m" => Some(config_data(
            name,
            ProtocolFamily::AnthropicMessages,
            1_000_000,
            20,
            0,
            true,
            true,
        )),
        "gpt5_270k" => Some(config_data(
            name,
            ProtocolFamily::OpenAiResponses,
            270_000,
            20,
            0,
            false,
            true,
        )),
        "gpt5_1m" => Some(config_data(
            name,
            ProtocolFamily::OpenAiResponses,
            922_000,
            20,
            0,
            false,
            true,
        )),
        "deepseek_v4_400k" => Some(config_data(
            name,
            ProtocolFamily::OpenAiChatCompletions,
            400_000,
            0,
            0,
            false,
            false,
        )),
        "deepseek_v4_1m" => Some(config_data(
            name,
            ProtocolFamily::OpenAiChatCompletions,
            1_000_000,
            0,
            0,
            false,
            false,
        )),
        "mimo_v2_5_1m" => Some(config_data(
            name,
            ProtocolFamily::OpenAiChatCompletions,
            1_000_000,
            0,
            0,
            false,
            false,
        )),
        "mimo_v2_5_pro_1m" => Some(config_data(
            name,
            ProtocolFamily::OpenAiChatCompletions,
            1_000_000,
            0,
            0,
            false,
            false,
        )),
        "gemini_200k" => Some(config_data(
            name,
            ProtocolFamily::GeminiGenerateContent,
            200_000,
            20,
            1,
            true,
            true,
        )),
        "gemini_1m" => Some(config_data(
            name,
            ProtocolFamily::GeminiGenerateContent,
            1_000_000,
            20,
            1,
            true,
            true,
        )),
        _ => None,
    }
}

fn anthropic_adaptive(
    effort: &str,
    max_tokens: u32,
    use_1m: bool,
    use_interleaved: bool,
    use_context_management: bool,
) -> ModelSettings {
    let mut settings = ModelSettings {
        max_tokens: Some(max_tokens),
        thinking: Some(ThinkingSettings {
            effort: effort.to_string(),
            budget_tokens: None,
            mode: Some("adaptive".to_string()),
            include_thoughts: None,
            summary: None,
        }),
        provider_options: Some(json!({
            "anthropic_effort": effort,
            "anthropic_cache_instructions": true,
            "anthropic_cache_tool_definitions": true,
            "anthropic_cache_response": true,
            "anthropic_cache_messages": true,
        })),
        ..ModelSettings::default()
    };
    apply_anthropic_betas(
        &mut settings,
        use_1m,
        use_interleaved,
        use_context_management,
    );
    if use_context_management {
        settings.extra_body.insert(
            "context_management".to_string(),
            default_context_management(),
        );
    }
    settings
}

fn anthropic_off(
    use_1m: bool,
    use_interleaved: bool,
    use_context_management: bool,
) -> ModelSettings {
    let mut settings = ModelSettings {
        thinking: Some(ThinkingSettings {
            effort: "off".to_string(),
            budget_tokens: None,
            mode: Some("disabled".to_string()),
            include_thoughts: None,
            summary: None,
        }),
        provider_options: Some(json!({
            "anthropic_cache_instructions": true,
            "anthropic_cache_tool_definitions": true,
            "anthropic_cache_response": true,
            "anthropic_cache_messages": true,
        })),
        ..ModelSettings::default()
    };
    apply_anthropic_betas(
        &mut settings,
        use_1m,
        use_interleaved,
        use_context_management,
    );
    if use_context_management {
        settings.extra_body.insert(
            "context_management".to_string(),
            default_context_management(),
        );
    }
    settings
}

fn apply_anthropic_betas(
    settings: &mut ModelSettings,
    use_1m: bool,
    use_interleaved: bool,
    use_context_management: bool,
) {
    let mut betas = Vec::new();
    if use_1m {
        betas.push(ANTHROPIC_1M_BETA);
    }
    if use_interleaved {
        betas.push(ANTHROPIC_INTERLEAVED_BETA);
    }
    if use_context_management {
        betas.push(ANTHROPIC_CONTEXT_MANAGEMENT_BETA);
    }
    if !betas.is_empty() {
        settings
            .extra_headers
            .insert("anthropic-beta".to_string(), betas.join(","));
    }
}

fn default_context_management() -> Value {
    json!({
        "edits": [{"type": "clear_thinking_20251015", "keep": "all"}]
    })
}

fn openai_chat(effort: &str, max_tokens: u32) -> ModelSettings {
    ModelSettings {
        max_tokens: Some(max_tokens),
        thinking: Some(ThinkingSettings {
            effort: effort.to_string(),
            budget_tokens: None,
            mode: None,
            include_thoughts: None,
            summary: None,
        }),
        ..ModelSettings::default()
    }
}

fn openai_responses(
    effort: &str,
    summary: &str,
    max_tokens: u32,
    service_tier: Option<ServiceTier>,
) -> ModelSettings {
    ModelSettings {
        max_tokens: Some(max_tokens),
        thinking: Some(ThinkingSettings {
            effort: effort.to_string(),
            budget_tokens: None,
            mode: None,
            include_thoughts: None,
            summary: Some(summary.to_string()),
        }),
        service_tier,
        provider_options: Some(json!({"store": false})),
        ..ModelSettings::default()
    }
}

fn openai_compatible_thinking(
    effort: &str,
    max_tokens: Option<u32>,
    enabled: bool,
) -> ModelSettings {
    let mut extra_body = Map::new();
    extra_body.insert(
        "thinking".to_string(),
        json!({"type": if enabled { "enabled" } else { "disabled" }}),
    );
    ModelSettings {
        max_tokens,
        thinking: enabled.then(|| ThinkingSettings {
            effort: effort.to_string(),
            budget_tokens: None,
            mode: Some("enabled".to_string()),
            include_thoughts: None,
            summary: None,
        }),
        extra_body,
        ..ModelSettings::default()
    }
}

fn mimo_v2_5() -> ModelSettings {
    let mut extra_body = Map::new();
    extra_body.insert("thinking".to_string(), json!({"type": "enabled"}));
    ModelSettings {
        extra_body,
        ..ModelSettings::default()
    }
}

fn gemini_budget(thinking_budget: u32, max_tokens: u32) -> ModelSettings {
    ModelSettings {
        max_tokens: Some(max_tokens),
        thinking: Some(ThinkingSettings {
            effort: String::new(),
            budget_tokens: Some(thinking_budget),
            mode: None,
            include_thoughts: Some(true),
            summary: None,
        }),
        ..ModelSettings::default()
    }
}

fn gemini_level(level: &str, max_tokens: u32) -> ModelSettings {
    ModelSettings {
        max_tokens: Some(max_tokens),
        thinking: Some(ThinkingSettings {
            effort: level.to_string(),
            budget_tokens: None,
            mode: None,
            include_thoughts: Some(true),
            summary: None,
        }),
        ..ModelSettings::default()
    }
}

fn config_data(
    name: &str,
    protocol: ProtocolFamily,
    context_window: u32,
    max_images: u32,
    max_videos: u32,
    supports_gif: bool,
    split_large_images: bool,
) -> ModelConfigPresetData {
    let mut profile = ModelProfile::for_protocol(protocol);
    profile.supports_image_input = max_images > 0;
    profile.supports_video_input = max_videos > 0;
    profile.supports_audio_input = matches!(protocol, ProtocolFamily::GeminiGenerateContent);
    profile.supports_document_input = matches!(
        protocol,
        ProtocolFamily::AnthropicMessages | ProtocolFamily::GeminiGenerateContent
    );
    ModelConfigPresetData {
        name: name.to_string(),
        protocol,
        context_window,
        max_images,
        max_videos,
        supports_gif,
        split_large_images,
        image_split_max_height: 4096,
        image_split_overlap: 50,
        profile,
    }
}

fn model_settings_alias(name: &str) -> &str {
    MODEL_SETTINGS_ALIASES
        .iter()
        .find_map(|(alias, canonical)| (*alias == name).then_some(*canonical))
        .unwrap_or(name)
}

fn model_config_alias(name: &str) -> &str {
    MODEL_CONFIG_ALIASES
        .iter()
        .find_map(|(alias, canonical)| (*alias == name).then_some(*canonical))
        .unwrap_or(name)
}

const MODEL_SETTINGS_PRESETS: &[&str] = &[
    "anthropic_default",
    "anthropic_high",
    "anthropic_medium",
    "anthropic_low",
    "anthropic_off",
    "anthropic_adaptive_default",
    "anthropic_adaptive_xhigh",
    "anthropic_adaptive_high",
    "anthropic_adaptive_medium",
    "anthropic_adaptive_low",
    "anthropic_adaptive_1m_default",
    "anthropic_adaptive_1m_xhigh",
    "anthropic_adaptive_1m_high",
    "anthropic_adaptive_1m_medium",
    "anthropic_adaptive_1m_low",
    "anthropic_adaptive_cm_default",
    "anthropic_adaptive_cm_xhigh",
    "anthropic_adaptive_cm_high",
    "anthropic_adaptive_cm_medium",
    "anthropic_adaptive_cm_low",
    "anthropic_adaptive_1m_cm_default",
    "anthropic_adaptive_1m_cm_xhigh",
    "anthropic_adaptive_1m_cm_high",
    "anthropic_adaptive_1m_cm_medium",
    "anthropic_adaptive_1m_cm_low",
    "anthropic_default_interleaved_thinking",
    "anthropic_high_interleaved_thinking",
    "anthropic_medium_interleaved_thinking",
    "anthropic_low_interleaved_thinking",
    "anthropic_off_interleaved_thinking",
    "anthropic_1m_default",
    "anthropic_1m_high",
    "anthropic_1m_medium",
    "anthropic_1m_low",
    "anthropic_1m_off",
    "anthropic_1m_default_interleaved_thinking",
    "anthropic_1m_high_interleaved_thinking",
    "anthropic_1m_medium_interleaved_thinking",
    "anthropic_1m_low_interleaved_thinking",
    "anthropic_1m_off_interleaved_thinking",
    "anthropic_cm_default",
    "anthropic_cm_high",
    "anthropic_cm_medium",
    "anthropic_cm_low",
    "anthropic_cm_off",
    "anthropic_1m_cm_default",
    "anthropic_1m_cm_high",
    "anthropic_1m_cm_medium",
    "anthropic_1m_cm_low",
    "anthropic_1m_cm_off",
    "anthropic_cm_default_interleaved_thinking",
    "anthropic_cm_high_interleaved_thinking",
    "anthropic_cm_medium_interleaved_thinking",
    "anthropic_cm_low_interleaved_thinking",
    "anthropic_cm_off_interleaved_thinking",
    "anthropic_1m_cm_default_interleaved_thinking",
    "anthropic_1m_cm_high_interleaved_thinking",
    "anthropic_1m_cm_medium_interleaved_thinking",
    "anthropic_1m_cm_low_interleaved_thinking",
    "anthropic_1m_cm_off_interleaved_thinking",
    "openai_default",
    "openai_xhigh",
    "openai_high",
    "openai_medium",
    "openai_low",
    "openai_responses_default",
    "openai_responses_xhigh",
    "openai_responses_high",
    "openai_responses_medium",
    "openai_responses_low",
    "openai_responses_default_fast",
    "openai_responses_xhigh_fast",
    "openai_responses_high_fast",
    "openai_responses_medium_fast",
    "openai_responses_low_fast",
    "deepseek_v4_default",
    "deepseek_v4_high",
    "deepseek_v4_max",
    "deepseek_v4_off",
    "mimo_v2_5",
    "mimo_v2_5_pro",
    "gemini_thinking_budget_default",
    "gemini_thinking_budget_high",
    "gemini_thinking_budget_medium",
    "gemini_thinking_budget_low",
    "gemini_thinking_level_default",
    "gemini_thinking_level_high",
    "gemini_thinking_level_medium",
    "gemini_thinking_level_low",
    "gemini_thinking_level_minimal",
];

const MODEL_SETTINGS_ALIASES: &[(&str, &str)] = &[
    ("anthropic", "anthropic_default"),
    ("anthropic_adaptive", "anthropic_adaptive_default"),
    ("anthropic_adaptive_1m", "anthropic_adaptive_1m_default"),
    ("anthropic_adaptive_cm", "anthropic_adaptive_cm_default"),
    (
        "anthropic_adaptive_1m_cm",
        "anthropic_adaptive_1m_cm_default",
    ),
    (
        "anthropic_interleaved",
        "anthropic_default_interleaved_thinking",
    ),
    ("anthropic_1m", "anthropic_1m_default"),
    (
        "anthropic_1m_interleaved",
        "anthropic_1m_default_interleaved_thinking",
    ),
    ("anthropic_cm", "anthropic_cm_default"),
    ("anthropic_1m_cm", "anthropic_1m_cm_default"),
    (
        "anthropic_cm_interleaved",
        "anthropic_cm_default_interleaved_thinking",
    ),
    (
        "anthropic_1m_cm_interleaved",
        "anthropic_1m_cm_default_interleaved_thinking",
    ),
    ("openai", "openai_default"),
    ("openai_responses", "openai_responses_default"),
    ("deepseek", "deepseek_v4_default"),
    ("deepseek_v4", "deepseek_v4_default"),
    ("mimo", "mimo_v2_5_pro"),
    ("mimo_v2.5", "mimo_v2_5"),
    ("mimo_v2.5_pro", "mimo_v2_5_pro"),
    ("gemini_2.5", "gemini_thinking_budget_default"),
    ("gemini_3", "gemini_thinking_level_default"),
    ("gemini", "gemini_thinking_level_default"),
    ("high", "anthropic_high"),
    ("medium", "anthropic_medium"),
    ("low", "anthropic_low"),
];

const MODEL_CONFIG_PRESETS: &[&str] = &[
    "claude_200k",
    "claude_400k",
    "claude_1m",
    "gpt5_270k",
    "gpt5_1m",
    "deepseek_v4_400k",
    "deepseek_v4_1m",
    "mimo_v2_5_1m",
    "mimo_v2_5_pro_1m",
    "gemini_200k",
    "gemini_1m",
];

const MODEL_CONFIG_ALIASES: &[(&str, &str)] = &[
    ("claude", "claude_1m"),
    ("anthropic", "claude_1m"),
    ("anthropic_400k", "claude_400k"),
    ("gpt5", "gpt5_270k"),
    ("openai", "gpt5_270k"),
    ("deepseek", "deepseek_v4_1m"),
    ("deepseek_400k", "deepseek_v4_400k"),
    ("deepseek_v4", "deepseek_v4_1m"),
    ("mimo", "mimo_v2_5_pro_1m"),
    ("mimo_v2.5", "mimo_v2_5_1m"),
    ("mimo_v2.5_pro", "mimo_v2_5_pro_1m"),
    ("mimo_v2_5", "mimo_v2_5_1m"),
    ("mimo_v2_5_pro", "mimo_v2_5_pro_1m"),
    ("gemini", "gemini_200k"),
];

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::too_many_lines)]
    fn preset_enums_match_registries_and_serde_names() {
        let settings_presets = [
            ModelSettingsPreset::AnthropicDefault,
            ModelSettingsPreset::AnthropicHigh,
            ModelSettingsPreset::AnthropicMedium,
            ModelSettingsPreset::AnthropicLow,
            ModelSettingsPreset::AnthropicOff,
            ModelSettingsPreset::AnthropicAdaptiveDefault,
            ModelSettingsPreset::AnthropicAdaptiveXhigh,
            ModelSettingsPreset::AnthropicAdaptiveHigh,
            ModelSettingsPreset::AnthropicAdaptiveMedium,
            ModelSettingsPreset::AnthropicAdaptiveLow,
            ModelSettingsPreset::AnthropicAdaptive1mDefault,
            ModelSettingsPreset::AnthropicAdaptive1mXhigh,
            ModelSettingsPreset::AnthropicAdaptive1mHigh,
            ModelSettingsPreset::AnthropicAdaptive1mMedium,
            ModelSettingsPreset::AnthropicAdaptive1mLow,
            ModelSettingsPreset::AnthropicAdaptiveCmDefault,
            ModelSettingsPreset::AnthropicAdaptiveCmXhigh,
            ModelSettingsPreset::AnthropicAdaptiveCmHigh,
            ModelSettingsPreset::AnthropicAdaptiveCmMedium,
            ModelSettingsPreset::AnthropicAdaptiveCmLow,
            ModelSettingsPreset::AnthropicAdaptive1mCmDefault,
            ModelSettingsPreset::AnthropicAdaptive1mCmXhigh,
            ModelSettingsPreset::AnthropicAdaptive1mCmHigh,
            ModelSettingsPreset::AnthropicAdaptive1mCmMedium,
            ModelSettingsPreset::AnthropicAdaptive1mCmLow,
            ModelSettingsPreset::AnthropicDefaultInterleavedThinking,
            ModelSettingsPreset::AnthropicHighInterleavedThinking,
            ModelSettingsPreset::AnthropicMediumInterleavedThinking,
            ModelSettingsPreset::AnthropicLowInterleavedThinking,
            ModelSettingsPreset::AnthropicOffInterleavedThinking,
            ModelSettingsPreset::Anthropic1mDefault,
            ModelSettingsPreset::Anthropic1mHigh,
            ModelSettingsPreset::Anthropic1mMedium,
            ModelSettingsPreset::Anthropic1mLow,
            ModelSettingsPreset::Anthropic1mOff,
            ModelSettingsPreset::Anthropic1mDefaultInterleavedThinking,
            ModelSettingsPreset::Anthropic1mHighInterleavedThinking,
            ModelSettingsPreset::Anthropic1mMediumInterleavedThinking,
            ModelSettingsPreset::Anthropic1mLowInterleavedThinking,
            ModelSettingsPreset::Anthropic1mOffInterleavedThinking,
            ModelSettingsPreset::AnthropicCmDefault,
            ModelSettingsPreset::AnthropicCmHigh,
            ModelSettingsPreset::AnthropicCmMedium,
            ModelSettingsPreset::AnthropicCmLow,
            ModelSettingsPreset::AnthropicCmOff,
            ModelSettingsPreset::Anthropic1mCmDefault,
            ModelSettingsPreset::Anthropic1mCmHigh,
            ModelSettingsPreset::Anthropic1mCmMedium,
            ModelSettingsPreset::Anthropic1mCmLow,
            ModelSettingsPreset::Anthropic1mCmOff,
            ModelSettingsPreset::AnthropicCmDefaultInterleavedThinking,
            ModelSettingsPreset::AnthropicCmHighInterleavedThinking,
            ModelSettingsPreset::AnthropicCmMediumInterleavedThinking,
            ModelSettingsPreset::AnthropicCmLowInterleavedThinking,
            ModelSettingsPreset::AnthropicCmOffInterleavedThinking,
            ModelSettingsPreset::Anthropic1mCmDefaultInterleavedThinking,
            ModelSettingsPreset::Anthropic1mCmHighInterleavedThinking,
            ModelSettingsPreset::Anthropic1mCmMediumInterleavedThinking,
            ModelSettingsPreset::Anthropic1mCmLowInterleavedThinking,
            ModelSettingsPreset::Anthropic1mCmOffInterleavedThinking,
            ModelSettingsPreset::OpenAiDefault,
            ModelSettingsPreset::OpenAiXhigh,
            ModelSettingsPreset::OpenAiHigh,
            ModelSettingsPreset::OpenAiMedium,
            ModelSettingsPreset::OpenAiLow,
            ModelSettingsPreset::OpenAiResponsesDefault,
            ModelSettingsPreset::OpenAiResponsesXhigh,
            ModelSettingsPreset::OpenAiResponsesHigh,
            ModelSettingsPreset::OpenAiResponsesMedium,
            ModelSettingsPreset::OpenAiResponsesLow,
            ModelSettingsPreset::OpenAiResponsesDefaultFast,
            ModelSettingsPreset::OpenAiResponsesXhighFast,
            ModelSettingsPreset::OpenAiResponsesHighFast,
            ModelSettingsPreset::OpenAiResponsesMediumFast,
            ModelSettingsPreset::OpenAiResponsesLowFast,
            ModelSettingsPreset::DeepSeekV4Default,
            ModelSettingsPreset::DeepSeekV4High,
            ModelSettingsPreset::DeepSeekV4Max,
            ModelSettingsPreset::DeepSeekV4Off,
            ModelSettingsPreset::MimoV25,
            ModelSettingsPreset::MimoV25Pro,
            ModelSettingsPreset::GeminiThinkingBudgetDefault,
            ModelSettingsPreset::GeminiThinkingBudgetHigh,
            ModelSettingsPreset::GeminiThinkingBudgetMedium,
            ModelSettingsPreset::GeminiThinkingBudgetLow,
            ModelSettingsPreset::GeminiThinkingLevelDefault,
            ModelSettingsPreset::GeminiThinkingLevelHigh,
            ModelSettingsPreset::GeminiThinkingLevelMedium,
            ModelSettingsPreset::GeminiThinkingLevelLow,
            ModelSettingsPreset::GeminiThinkingLevelMinimal,
        ];
        let settings_names = settings_presets
            .iter()
            .map(|preset| preset.as_str())
            .collect::<Vec<_>>();
        assert_eq!(settings_names, MODEL_SETTINGS_PRESETS);
        for preset in settings_presets {
            let value = serde_json::to_value(preset).unwrap();
            assert_eq!(
                value,
                serde_json::Value::String(preset.as_str().to_string())
            );
            let parsed: ModelSettingsPreset = serde_json::from_value(value).unwrap();
            assert_eq!(parsed, preset);
        }

        let config_presets = [
            ModelConfigPreset::Claude200k,
            ModelConfigPreset::Claude400k,
            ModelConfigPreset::Claude1m,
            ModelConfigPreset::Gpt5_270k,
            ModelConfigPreset::Gpt5_1m,
            ModelConfigPreset::DeepSeekV4_400k,
            ModelConfigPreset::DeepSeekV4_1m,
            ModelConfigPreset::MimoV25_1m,
            ModelConfigPreset::MimoV25Pro1m,
            ModelConfigPreset::Gemini200k,
            ModelConfigPreset::Gemini1m,
        ];
        let config_names = config_presets
            .iter()
            .map(|preset| preset.as_str())
            .collect::<Vec<_>>();
        assert_eq!(config_names, MODEL_CONFIG_PRESETS);
        for preset in config_presets {
            let value = serde_json::to_value(preset).unwrap();
            assert_eq!(
                value,
                serde_json::Value::String(preset.as_str().to_string())
            );
            let parsed: ModelConfigPreset = serde_json::from_value(value).unwrap();
            assert_eq!(parsed, preset);
        }
    }
    #[test]
    fn resolves_model_settings_presets_and_aliases() {
        let anthropic = get_model_settings("anthropic").unwrap();
        assert_eq!(anthropic.max_tokens, Some(32 * K_TOKENS));
        assert_eq!(
            anthropic.thinking.unwrap().mode.as_deref(),
            Some("adaptive")
        );

        let fast = get_model_settings("openai_responses_high_fast").unwrap();
        assert_eq!(fast.service_tier, Some(ServiceTier::Priority));
        assert_eq!(fast.thinking.unwrap().summary.as_deref(), Some("detailed"));

        let gemini = get_model_settings("gemini_thinking_level_minimal").unwrap();
        assert_eq!(gemini.thinking.unwrap().effort, "MINIMAL");
    }

    #[test]
    fn resolves_model_config_presets_and_aliases() {
        let claude = get_model_config("claude").unwrap();
        assert_eq!(claude.context_window, 1_000_000);
        assert!(claude.profile.supports_document_input);

        let gemini = get_model_config("gemini").unwrap();
        assert_eq!(gemini.max_videos, 1);
        assert!(gemini.profile.supports_audio_input);
    }

    #[test]
    fn builds_runtime_preset_provider_alias() {
        let preset = model_runtime_preset(
            "claude-sonnet",
            "anthropic",
            "claude-sonnet-4-5",
            "anthropic_high",
            "claude_200k",
        )
        .unwrap();
        let alias = preset.provider_alias(anthropic_http_config("test-key"));
        assert_eq!(alias.alias, "claude-sonnet");
        assert_eq!(alias.model_name, "claude-sonnet-4-5");
        assert!(alias.default_settings.unwrap().thinking.is_some());
        assert!(alias.profile.unwrap().supports_document_input);
    }
}
