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
        ModelSettingsPreset::OpenAiResponsesMax,
        ModelSettingsPreset::OpenAiResponsesHigh,
        ModelSettingsPreset::OpenAiResponsesMedium,
        ModelSettingsPreset::OpenAiResponsesLow,
        ModelSettingsPreset::OpenAiResponsesPro,
        ModelSettingsPreset::OpenAiResponsesDefaultFast,
        ModelSettingsPreset::OpenAiResponsesXhighFast,
        ModelSettingsPreset::OpenAiResponsesMaxFast,
        ModelSettingsPreset::OpenAiResponsesHighFast,
        ModelSettingsPreset::OpenAiResponsesMediumFast,
        ModelSettingsPreset::OpenAiResponsesLowFast,
        ModelSettingsPreset::DeepSeekV4Default,
        ModelSettingsPreset::DeepSeekV4High,
        ModelSettingsPreset::DeepSeekV4Max,
        ModelSettingsPreset::DeepSeekV4Off,
        ModelSettingsPreset::Grok45Default,
        ModelSettingsPreset::Grok45High,
        ModelSettingsPreset::Grok45Medium,
        ModelSettingsPreset::Grok45Low,
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
        ModelConfigPreset::Gpt5_350k,
        ModelConfigPreset::Gpt5_1m,
        ModelConfigPreset::DeepSeekV4_400k,
        ModelConfigPreset::DeepSeekV4_1m,
        ModelConfigPreset::Grok45_500k,
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
    let fast_thinking = fast.thinking.unwrap();
    assert_eq!(fast_thinking.summary.as_deref(), Some("detailed"));
    assert!(fast_thinking.mode.is_none());

    let max = get_model_settings("openai_responses_max").unwrap();
    assert_eq!(max.max_tokens, Some(128 * K_TOKENS));
    assert_eq!(max.thinking.unwrap().effort, "max");

    let pro = get_model_settings("openai_responses_pro").unwrap();
    assert_eq!(pro.max_tokens, Some(16 * K_TOKENS));
    let pro_thinking = pro.thinking.unwrap();
    assert_eq!(pro_thinking.mode.as_deref(), Some("pro"));
    assert_eq!(pro_thinking.effort, "medium");
    assert_eq!(pro_thinking.summary.as_deref(), Some("auto"));

    let grok = get_model_settings("grok").unwrap();
    assert_eq!(grok.max_tokens, Some(32 * K_TOKENS));
    assert_eq!(grok.thinking.unwrap().effort, "high");

    let gemini = get_model_settings("gemini_thinking_level_minimal").unwrap();
    assert_eq!(gemini.thinking.unwrap().effort, "MINIMAL");
}

#[test]
fn resolves_model_config_presets_and_aliases() {
    let claude = get_model_config("claude").unwrap();
    assert_eq!(claude.context_window, 1_000_000);
    assert!(claude.profile.supports_document_input);

    let gpt5_350k = get_model_config("gpt5_350k").unwrap();
    assert_eq!(gpt5_350k.context_window, 350_000);
    assert_eq!(gpt5_350k.max_images, 20);
    assert!(gpt5_350k.profile.supports_image_input);
    assert!(gpt5_350k.profile.supports_image_output);

    let grok = get_model_config("grok-4.5").unwrap();
    assert_eq!(grok.context_window, 500_000);
    assert_eq!(grok.max_images, 20);
    assert!(grok.profile.supports_image_input);
    assert!(grok.profile.thinking_always_enabled);

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

    let grok = model_runtime_preset("grok", "xai", "grok-4.5", "grok", "grok-4.5").unwrap();
    let alias = grok.provider_alias(xai_responses_http_config("test-key"));
    assert_eq!(alias.alias, "grok");
    assert_eq!(alias.provider_name, "xai");
    assert_eq!(alias.model_name, "grok-4.5");
    assert_eq!(alias.http.endpoint_url(), "https://api.x.ai/v1/responses");
    assert!(alias.default_settings.unwrap().thinking.is_some());
    assert!(alias.profile.unwrap().thinking_always_enabled);
}
