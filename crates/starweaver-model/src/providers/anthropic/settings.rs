//! Anthropic settings and cache-control mapping.

use serde_json::{Value, json};

use crate::{
    ModelError, ModelSettings,
    adapter::ToolDefinition,
    providers::{insert_nonempty_description, provider_tool_schema_without_meta},
};

pub(super) fn apply_anthropic_settings(
    request: &mut serde_json::Map<String, Value>,
    settings: Option<&ModelSettings>,
) -> Result<(), ModelError> {
    let Some(settings) = settings else {
        return Ok(());
    };
    let automatic_cache = anthropic_cache_ttl(Some(settings), "anthropic_cache");
    let message_cache = anthropic_cache_ttl(Some(settings), "anthropic_cache_messages");
    if automatic_cache.is_some() && message_cache.is_some() {
        return Err(ModelError::MessageMapping(
            "anthropic_cache and anthropic_cache_messages cannot both be enabled".to_string(),
        ));
    }
    if let Some(ttl) = automatic_cache {
        request.insert("cache_control".to_string(), anthropic_cache_control(ttl));
    }
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
    Ok(())
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

pub(super) fn apply_anthropic_message_cache(
    request: &mut serde_json::Map<String, Value>,
    settings: Option<&ModelSettings>,
) -> Result<(), ModelError> {
    let Some(ttl) = anthropic_cache_ttl(settings, "anthropic_cache_messages") else {
        return Ok(());
    };
    let Some(messages) = request.get_mut("messages").and_then(Value::as_array_mut) else {
        return Ok(());
    };
    let Some(target) = messages.iter_mut().rev().find_map(|message| {
        message
            .get_mut("content")
            .and_then(Value::as_array_mut)
            .and_then(|content| {
                content
                    .iter_mut()
                    .rev()
                    .find(|block| anthropic_block_is_cacheable(block))
            })
    }) else {
        return Ok(());
    };
    if anthropic_cache_control_ttl(target)?.is_none() {
        target["cache_control"] = anthropic_cache_control(ttl);
    }
    Ok(())
}

pub(super) fn validate_anthropic_cache_points(
    request: &serde_json::Map<String, Value>,
) -> Result<(), ModelError> {
    let automatic_ttl = request
        .get("cache_control")
        .map(anthropic_cache_control_value_ttl)
        .transpose()?;

    if let Some(automatic_ttl) = automatic_ttl
        && let Some(target) = last_anthropic_cacheable_block(request)
        && let Some(explicit_ttl) = anthropic_cache_control_ttl(target)?
        && explicit_ttl != automatic_ttl
    {
        return Err(ModelError::MessageMapping(format!(
            "anthropic_cache TTL {automatic_ttl} conflicts with the explicit {explicit_ttl} cache point on the last cacheable prompt block"
        )));
    }

    let mut saw_five_minute_point = false;
    for blocks in [
        request.get("tools").and_then(Value::as_array),
        request.get("system").and_then(Value::as_array),
    ]
    .into_iter()
    .flatten()
    {
        validate_anthropic_cache_ttl_order(blocks, &mut saw_five_minute_point)?;
    }
    if let Some(messages) = request.get("messages").and_then(Value::as_array) {
        for message in messages {
            if let Some(blocks) = message.get("content").and_then(Value::as_array) {
                validate_anthropic_cache_ttl_order(blocks, &mut saw_five_minute_point)?;
            }
        }
    }
    if let Some(ttl) = automatic_ttl {
        validate_anthropic_ttl_position(ttl, &mut saw_five_minute_point)?;
    }
    Ok(())
}

fn validate_anthropic_cache_ttl_order(
    blocks: &[Value],
    saw_five_minute_point: &mut bool,
) -> Result<(), ModelError> {
    for block in blocks {
        if let Some(ttl) = anthropic_cache_control_ttl(block)? {
            validate_anthropic_ttl_position(ttl, saw_five_minute_point)?;
        }
    }
    Ok(())
}

fn validate_anthropic_ttl_position(
    ttl: &str,
    saw_five_minute_point: &mut bool,
) -> Result<(), ModelError> {
    match ttl {
        "5m" => *saw_five_minute_point = true,
        "1h" if *saw_five_minute_point => {
            return Err(ModelError::MessageMapping(
                "Anthropic 1h cache points must precede all 5m cache points".to_string(),
            ));
        }
        "1h" => {}
        _ => unreachable!("cache TTL was validated before ordering"),
    }
    Ok(())
}

fn last_anthropic_cacheable_block(request: &serde_json::Map<String, Value>) -> Option<&Value> {
    request
        .get("messages")
        .and_then(Value::as_array)
        .and_then(|messages| {
            messages.iter().rev().find_map(|message| {
                message
                    .get("content")
                    .and_then(Value::as_array)
                    .and_then(|blocks| {
                        blocks
                            .iter()
                            .rev()
                            .find(|block| anthropic_block_is_cacheable(block))
                    })
            })
        })
        .or_else(|| {
            request
                .get("system")
                .and_then(Value::as_array)
                .and_then(|blocks| {
                    blocks
                        .iter()
                        .rev()
                        .find(|block| anthropic_block_is_cacheable(block))
                })
        })
        .or_else(|| {
            request
                .get("tools")
                .and_then(Value::as_array)
                .and_then(|blocks| blocks.last())
        })
}

pub(super) fn anthropic_block_is_cacheable(block: &Value) -> bool {
    matches!(
        block.get("type").and_then(Value::as_str),
        Some("text" | "image" | "document" | "tool_use" | "tool_result")
    )
}

fn anthropic_cache_control_ttl(block: &Value) -> Result<Option<&str>, ModelError> {
    let Some(cache_control) = block.get("cache_control") else {
        return Ok(None);
    };
    anthropic_cache_control_value_ttl(cache_control).map(Some)
}

fn anthropic_cache_control_value_ttl(cache_control: &Value) -> Result<&str, ModelError> {
    let Some(cache_control) = cache_control.as_object() else {
        return Err(ModelError::MessageMapping(
            "Anthropic cache_control must be an object".to_string(),
        ));
    };
    if cache_control.get("type").and_then(Value::as_str) != Some("ephemeral") {
        return Err(ModelError::MessageMapping(
            "Anthropic cache_control.type must be ephemeral".to_string(),
        ));
    }
    match cache_control.get("ttl").and_then(Value::as_str) {
        None | Some("5m") => Ok("5m"),
        Some("1h") => Ok("1h"),
        Some(ttl) => Err(ModelError::MessageMapping(format!(
            "unsupported Anthropic cache_control TTL {ttl}; expected 5m or 1h"
        ))),
    }
}

pub(super) fn limit_anthropic_cache_points(
    request: &mut serde_json::Map<String, Value>,
) -> Result<(), ModelError> {
    let max_explicit = if request.contains_key("cache_control") {
        3
    } else {
        4
    };
    let system_points = request
        .get("system")
        .and_then(Value::as_array)
        .map_or(0, |blocks| {
            blocks
                .iter()
                .filter(|block| block.get("cache_control").is_some())
                .count()
        });
    let tool_points = request
        .get("tools")
        .and_then(Value::as_array)
        .map_or(0, |tools| {
            tools
                .iter()
                .filter(|tool| tool.get("cache_control").is_some())
                .count()
        });
    let reserved = system_points + tool_points;
    if reserved > max_explicit {
        return Err(ModelError::MessageMapping(format!(
            "Anthropic system and tool definitions use {reserved} cache points, exceeding the available {max_explicit}"
        )));
    }

    let mut remaining = max_explicit - reserved;
    if let Some(messages) = request.get_mut("messages").and_then(Value::as_array_mut) {
        for message in messages.iter_mut().rev() {
            let Some(content) = message.get_mut("content").and_then(Value::as_array_mut) else {
                continue;
            };
            for block in content.iter_mut().rev() {
                if block.get("cache_control").is_none() {
                    continue;
                }
                if remaining > 0 {
                    remaining -= 1;
                } else if let Some(block) = block.as_object_mut() {
                    block.remove("cache_control");
                }
            }
        }
    }
    Ok(())
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
