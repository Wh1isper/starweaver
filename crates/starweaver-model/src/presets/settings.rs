//! Built-in model settings preset resolution.

use serde_json::{Map, Value, json};

use crate::{ModelSettings, ServiceTier, ThinkingSettings};

pub(super) const K_TOKENS: u32 = 1024;
const ANTHROPIC_1M_BETA: &str = "context-1m-2025-08-07";
const ANTHROPIC_INTERLEAVED_BETA: &str = "interleaved-thinking-2025-05-14";
const ANTHROPIC_CONTEXT_MANAGEMENT_BETA: &str = "context-management-2025-06-27";

#[allow(clippy::match_same_arms)]
pub(super) fn model_settings_by_name(name: &str) -> Option<ModelSettings> {
    if let Some(spec) = parse_anthropic_preset(name)
        .or_else(|| parse_anthropic_preset(anthropic_preset_alias(name)))
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
        "openai_responses_max" => Some(openai_responses("max", "detailed", 128 * K_TOKENS, None)),
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
        "openai_responses_max_fast" => Some(openai_responses(
            "max",
            "detailed",
            128 * K_TOKENS,
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
        "deepseek_v4_default" | "deepseek_v4_high" => {
            Some(openai_protocol_thinking("high", Some(128 * K_TOKENS), true))
        }
        "deepseek_v4_max" => Some(openai_protocol_thinking("max", Some(384 * K_TOKENS), true)),
        "deepseek_v4_off" => Some(openai_protocol_thinking(
            "high",
            Some(128 * K_TOKENS),
            false,
        )),
        "grok_4_5_default" | "grok_4_5_high" => Some(xai_responses("high", 32 * K_TOKENS)),
        "grok_4_5_medium" => Some(xai_responses("medium", 16 * K_TOKENS)),
        "grok_4_5_low" => Some(xai_responses("low", 8 * K_TOKENS)),
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

fn anthropic_preset_alias(name: &str) -> &str {
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

fn xai_responses(effort: &str, max_tokens: u32) -> ModelSettings {
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

fn openai_protocol_thinking(effort: &str, max_tokens: Option<u32>, enabled: bool) -> ModelSettings {
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
