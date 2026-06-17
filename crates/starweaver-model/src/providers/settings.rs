//! Provider request settings helpers.

use serde_json::{json, Value};

use crate::{transport::MaxTokensParameter, ModelSettings};

#[cfg(test)]
pub fn apply_common_settings(
    target: &mut serde_json::Map<String, Value>,
    settings: Option<&ModelSettings>,
) {
    apply_common_settings_with_max_tokens(target, settings, MaxTokensParameter::MaxTokens);
}

pub fn apply_common_settings_with_max_tokens(
    target: &mut serde_json::Map<String, Value>,
    settings: Option<&ModelSettings>,
    max_tokens_parameter: MaxTokensParameter,
) {
    apply_common_settings_with_options(target, settings, max_tokens_parameter, true);
}

pub fn apply_common_settings_without_seed(
    target: &mut serde_json::Map<String, Value>,
    settings: Option<&ModelSettings>,
    max_tokens_parameter: MaxTokensParameter,
) {
    apply_common_settings_with_options(target, settings, max_tokens_parameter, false);
}

fn apply_common_settings_with_options(
    target: &mut serde_json::Map<String, Value>,
    settings: Option<&ModelSettings>,
    max_tokens_parameter: MaxTokensParameter,
    include_seed: bool,
) {
    let max_tokens_key = match max_tokens_parameter {
        MaxTokensParameter::Default | MaxTokensParameter::MaxTokens => Some("max_tokens"),
        MaxTokensParameter::MaxOutputTokens => Some("max_output_tokens"),
        MaxTokensParameter::MaxCompletionTokens => Some("max_completion_tokens"),
        MaxTokensParameter::Omit => None,
    };
    apply_common_settings_inner(target, settings, max_tokens_key, include_seed);
}

fn apply_common_settings_inner(
    target: &mut serde_json::Map<String, Value>,
    settings: Option<&ModelSettings>,
    max_tokens_key: Option<&str>,
    include_seed: bool,
) {
    if let Some(settings) = settings {
        if let (Some(key), Some(max_tokens)) = (max_tokens_key, settings.max_tokens) {
            target.insert(key.to_string(), json!(max_tokens));
        }
        if let Some(temperature) = settings.temperature {
            target.insert("temperature".to_string(), json!(temperature));
        }
        if let Some(top_p) = settings.top_p {
            target.insert("top_p".to_string(), json!(top_p));
        }
        if let Some(presence_penalty) = settings.presence_penalty {
            target.insert("presence_penalty".to_string(), json!(presence_penalty));
        }
        if let Some(frequency_penalty) = settings.frequency_penalty {
            target.insert("frequency_penalty".to_string(), json!(frequency_penalty));
        }
        if !settings.logit_bias.is_empty() {
            target.insert("logit_bias".to_string(), json!(settings.logit_bias));
        }
        if !settings.stop_sequences.is_empty() {
            target.insert("stop".to_string(), json!(settings.stop_sequences));
        }
        if let Some(seed) = settings.seed.filter(|_| include_seed) {
            target.insert("seed".to_string(), json!(seed));
        }
        if let Some(parallel_tool_calls) = settings.parallel_tool_calls {
            target.insert(
                "parallel_tool_calls".to_string(),
                json!(parallel_tool_calls),
            );
        }
        if let Some(thinking) = &settings.thinking {
            target.insert("reasoning_effort".to_string(), json!(thinking.effort));
        }
        if let Some(service_tier) = &settings.service_tier {
            target.insert("service_tier".to_string(), json!(service_tier));
        }
        if let Some(options) = settings
            .provider_options
            .as_ref()
            .and_then(Value::as_object)
        {
            for (key, value) in options {
                target.insert(key.clone(), value.clone());
            }
        }
    }
}
