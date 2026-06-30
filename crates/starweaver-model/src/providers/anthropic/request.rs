//! Anthropic request mapping.

use serde_json::{json, Value};

use crate::{
    adapter::ToolDefinition,
    message::{ModelMessage, ModelRequestPart, ModelResponsePart},
    providers::{collect_system_parts_and_non_system, SystemInstructionPart},
    ModelError, ModelSettings,
};

use super::{
    content::{anthropic_content_from_content, anthropic_tool_result},
    settings::{
        anthropic_cache_control, anthropic_cache_ttl, append_anthropic_tools,
        apply_anthropic_settings,
    },
};

pub(super) fn build_request(
    model: &str,
    messages: &[ModelMessage],
    settings: Option<&ModelSettings>,
    tools: &[ToolDefinition],
) -> Result<Value, ModelError> {
    let (system, rest) = collect_system_parts_and_non_system(messages);
    let mut wire_messages = Vec::new();

    for message in rest {
        match message {
            ModelMessage::Request(request) => {
                let mut content = Vec::new();
                for part in &request.parts {
                    match part {
                        ModelRequestPart::UserPrompt {
                            content: user_content,
                            ..
                        } => {
                            content.extend(anthropic_content_from_content(user_content)?);
                        }
                        ModelRequestPart::ToolReturn(tool_return) => {
                            content.push(anthropic_tool_result(tool_return));
                        }
                        ModelRequestPart::RetryPrompt { text, .. } => {
                            content.push(json!({"type": "text", "text": text}));
                        }
                        ModelRequestPart::SystemPrompt { .. }
                        | ModelRequestPart::Instruction { .. } => {}
                    }
                }
                if !content.is_empty() {
                    wire_messages.push(json!({"role": "user", "content": content}));
                }
            }
            ModelMessage::Response(response) => {
                let mut content = Vec::new();
                for part in &response.parts {
                    match part {
                        ModelResponsePart::Text { text }
                        | ModelResponsePart::ProviderText { text, .. } => {
                            content.push(json!({"type": "text", "text": text}));
                        }
                        ModelResponsePart::ToolCall(call)
                        | ModelResponsePart::ProviderToolCall { call, .. } => {
                            content.push(json!({
                                "type": "tool_use",
                                "id": call.id,
                                "name": call.name,
                                "input": call.arguments,
                            }));
                        }
                        ModelResponsePart::Thinking { text, .. } if !text.is_empty() => {
                            content.push(json!({
                                "type": "text",
                                "text": format!("<think>\n{text}\n</think>"),
                            }));
                        }
                        ModelResponsePart::ProviderThinking {
                            text,
                            signature,
                            provider,
                        } => {
                            if provider.is_provider("anthropic") {
                                let mut thinking = json!({"type": "thinking", "thinking": text});
                                if let Some(signature) = signature {
                                    thinking["signature"] = json!(signature);
                                }
                                content.push(thinking);
                            } else if !text.is_empty() {
                                content.push(json!({
                                    "type": "text",
                                    "text": format!("<think>\n{text}\n</think>"),
                                }));
                            }
                        }
                        ModelResponsePart::ProviderOpaque {
                            item_type,
                            payload,
                            provider,
                        } if provider.is_provider("anthropic")
                            && anthropic_opaque_replay_item(item_type, payload) =>
                        {
                            content.push(payload.clone());
                        }
                        _ => {}
                    }
                }
                if !content.is_empty() {
                    wire_messages.push(json!({"role": "assistant", "content": content}));
                }
            }
        }
    }

    let mut request = serde_json::Map::new();
    request.insert("model".to_string(), json!(model));
    request.insert("messages".to_string(), json!(wire_messages));
    request.insert(
        "max_tokens".to_string(),
        json!(settings.and_then(|s| s.max_tokens).unwrap_or(1024)),
    );
    if let Some(system) = anthropic_system_value(&system, settings) {
        request.insert("system".to_string(), system);
    }
    apply_anthropic_settings(&mut request, settings);
    append_anthropic_tools(&mut request, tools, settings);
    Ok(Value::Object(request))
}

fn anthropic_opaque_replay_item(item_type: &str, payload: &Value) -> bool {
    item_type == "redacted_thinking"
        && payload.get("type").and_then(Value::as_str) == Some("redacted_thinking")
}

fn anthropic_system_value(
    system: &[SystemInstructionPart],
    settings: Option<&ModelSettings>,
) -> Option<Value> {
    if system.is_empty() {
        return None;
    }
    let Some(ttl) = anthropic_cache_ttl(settings, "anthropic_cache_instructions") else {
        return Some(json!(system
            .iter()
            .map(|part| part.text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n")));
    };

    let mut blocks = system
        .iter()
        .map(|part| json!({"type": "text", "text": part.text}))
        .collect::<Vec<_>>();
    if let Some(index) = instruction_cache_index(system) {
        blocks[index]["cache_control"] = anthropic_cache_control(ttl);
    }
    Some(Value::Array(blocks))
}

fn instruction_cache_index(system: &[SystemInstructionPart]) -> Option<usize> {
    if let Some(first_dynamic) = system.iter().position(|part| part.dynamic) {
        return (0..first_dynamic)
            .rev()
            .find(|index| !system[*index].dynamic);
    }
    system.len().checked_sub(1)
}
