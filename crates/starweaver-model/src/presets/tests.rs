use super::{
    registry::{MODEL_CONFIG_PRESETS, MODEL_SETTINGS_PRESETS},
    settings::K_TOKENS,
    *,
};
use crate::ServiceTier;

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
