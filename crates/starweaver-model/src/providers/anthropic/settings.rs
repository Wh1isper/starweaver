//! Anthropic settings and cache-control mapping.

use serde_json::{Value, json};

use crate::{
    ModelSettings,
    adapter::ToolDefinition,
    providers::{insert_nonempty_description, provider_tool_schema_without_meta},
};

pub(super) fn apply_anthropic_settings(
    request: &mut serde_json::Map<String, Value>,
    settings: Option<&ModelSettings>,
) {
    let Some(settings) = settings else {
        return;
    };
    if let Some(thinking) = &settings.thinking {
        let thinking_mode = thinking.mode.as_deref().unwrap_or("enabled");
        let mut payload = serde_json::Map::new();
        payload.insert("type".to_string(), json!(thinking_mode));
        if thinking_mode == "enabled" {
            payload.insert(
                "budget_tokens".to_string(),
                json!(thinking.budget_tokens.unwrap_or(1024)),
            );
        }
        if thinking_mode == "adaptive" {
            payload.insert("display".to_string(), json!("summarized"));
            if !thinking.effort.is_empty() {
                let output_config = request
                    .entry("output_config".to_string())
                    .or_insert_with(|| Value::Object(serde_json::Map::new()));
                if let Some(output_config) = output_config.as_object_mut() {
                    output_config.insert("effort".to_string(), json!(thinking.effort));
                }
            }
        }
        request.insert("thinking".to_string(), Value::Object(payload));
    }
    if let Some(temperature) = settings.temperature {
        request.insert("temperature".to_string(), json!(temperature));
    }
    if let Some(top_p) = settings.top_p {
        request.insert("top_p".to_string(), json!(top_p));
    }
    if let Some(top_k) = settings.top_k {
        request.insert("top_k".to_string(), json!(top_k));
    }
    if !settings.stop_sequences.is_empty() {
        request.insert("stop_sequences".to_string(), json!(settings.stop_sequences));
    }
    if let Some(anthropic) = &settings.provider_settings.anthropic {
        if let Some(metadata) = &anthropic.metadata {
            request.insert("metadata".to_string(), metadata.clone());
        }
        if let Some(context_management) = &anthropic.context_management {
            request.insert("context_management".to_string(), context_management.clone());
        }
        if let Some(container) = &anthropic.container {
            request.insert("container".to_string(), json!(container));
        }
        if let Some(service_tier) = &anthropic.service_tier {
            request.insert("service_tier".to_string(), json!(service_tier));
        }
    }
    if let Some(tool_choice) = settings.tool_choice.as_ref() {
        let mut choice = anthropic_tool_choice(tool_choice);
        if settings.parallel_tool_calls == Some(false)
            && !matches!(tool_choice, crate::settings::ToolChoice::None)
            && let Some(choice) = choice.as_object_mut()
        {
            choice.insert("disable_parallel_tool_use".to_string(), json!(true));
        }
        request.insert("tool_choice".to_string(), choice);
    } else if settings.parallel_tool_calls == Some(false) {
        request.insert(
            "tool_choice".to_string(),
            json!({"type": "auto", "disable_parallel_tool_use": true}),
        );
    }
    if let Some(options) = settings
        .provider_options
        .as_ref()
        .and_then(Value::as_object)
    {
        request.extend(
            options
                .iter()
                .filter(|(key, _)| !is_internal_anthropic_option(key))
                .map(|(key, value)| (key.clone(), value.clone())),
        );
    }
}

pub(super) fn append_anthropic_tools(
    request: &mut serde_json::Map<String, Value>,
    tools: &[ToolDefinition],
    settings: Option<&ModelSettings>,
) {
    if tools.is_empty() {
        return;
    }
    let mut definitions = tools
        .iter()
        .map(|tool| {
            let mut definition = serde_json::Map::new();
            definition.insert("name".to_string(), json!(tool.name));
            insert_nonempty_description(&mut definition, tool.description.as_ref());
            definition.insert(
                "input_schema".to_string(),
                provider_tool_schema_without_meta(&tool.parameters),
            );
            Value::Object(definition)
        })
        .collect::<Vec<_>>();
    if let Some(ttl) = anthropic_cache_ttl(settings, "anthropic_cache_tool_definitions")
        && let Some(last) = definitions.last_mut()
    {
        last["cache_control"] = anthropic_cache_control(ttl);
    }
    request.insert("tools".to_string(), Value::Array(definitions));
}

fn anthropic_tool_choice(choice: &crate::settings::ToolChoice) -> Value {
    match choice {
        crate::settings::ToolChoice::Auto | crate::settings::ToolChoice::ToolOrOutput { .. } => {
            json!({"type": "auto"})
        }
        crate::settings::ToolChoice::None => json!({"type": "none"}),
        crate::settings::ToolChoice::Required | crate::settings::ToolChoice::Tools { .. } => {
            json!({"type": "any"})
        }
        crate::settings::ToolChoice::Tool { name } => json!({"type": "tool", "name": name}),
    }
}

pub(super) fn anthropic_cache_ttl(
    settings: Option<&ModelSettings>,
    key: &str,
) -> Option<&'static str> {
    let value = settings
        .and_then(|settings| settings.provider_options.as_ref())
        .and_then(|options| options.get(key));
    match value {
        Some(Value::Bool(true)) => Some("5m"),
        Some(Value::String(value)) if value == "5m" => Some("5m"),
        Some(Value::String(value)) if value == "1h" => Some("1h"),
        _ => None,
    }
}

pub(super) fn anthropic_cache_control(ttl: &str) -> Value {
    json!({"type": "ephemeral", "ttl": ttl})
}

fn is_internal_anthropic_option(key: &str) -> bool {
    matches!(
        key,
        "anthropic_cache"
            | "anthropic_cache_instructions"
            | "anthropic_cache_tool_definitions"
            | "anthropic_cache_response"
            | "anthropic_cache_messages"
            | "anthropic_effort"
    )
}
