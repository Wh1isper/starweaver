use serde::{Deserialize, Serialize};

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
