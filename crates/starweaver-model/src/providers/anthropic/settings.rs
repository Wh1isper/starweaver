//! Anthropic settings and cache-control mapping.

use serde_json::{json, Value};

use crate::{
    adapter::ToolDefinition,
    providers::{insert_nonempty_description, provider_tool_schema_without_meta},
    ModelSettings,
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
                request.insert(
                    "output_config".to_string(),
                    json!({"effort": thinking.effort}),
                );
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
    if let Some(ttl) = anthropic_cache_ttl(settings, "anthropic_cache_tool_definitions") {
        if let Some(last) = definitions.last_mut() {
            last["cache_control"] = anthropic_cache_control(ttl);
        }
    }
    request.insert("tools".to_string(), Value::Array(definitions));
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
