//! Anthropic Messages wire mapper.

use serde_json::{json, Value};

use crate::{
    adapter::ToolDefinition,
    message::{
        FinishReason, ModelMessage, ModelRequestPart, ModelResponse, ModelResponsePart,
        ProviderInfo, ToolCallPart, ToolReturnPart,
    },
    providers::{
        collect_system_and_non_system, insert_optional_description, provider_tool_parameters,
        text_from_content, usage_from_named,
    },
    ModelError, ModelSettings,
};

/// Anthropic Messages wire mapper.
pub struct AnthropicMessagesAdapter;

impl AnthropicMessagesAdapter {
    /// Build a provider wire request.
    ///
    /// # Errors
    ///
    /// Returns an error when canonical history cannot be mapped into Anthropic messages.
    pub fn build_request(
        model: &str,
        messages: &[ModelMessage],
        settings: Option<&ModelSettings>,
        tools: &[ToolDefinition],
    ) -> Result<Value, ModelError> {
        let (system, rest) = collect_system_and_non_system(messages);
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
                                content.push(json!({"type": "text", "text": text_from_content(user_content)}));
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
                            ModelResponsePart::Text { text } => {
                                content.push(json!({"type": "text", "text": text}));
                            }
                            ModelResponsePart::ToolCall(call) => content.push(json!({
                                "type": "tool_use",
                                "id": call.id,
                                "name": call.name,
                                "input": call.arguments,
                            })),
                            ModelResponsePart::Thinking { text, signature } => {
                                let mut thinking = json!({"type": "thinking", "thinking": text});
                                if let Some(signature) = signature {
                                    thinking["signature"] = json!(signature);
                                }
                                content.push(thinking);
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
        if !system.is_empty() {
            request.insert("system".to_string(), json!(system.join("\n\n")));
        }
        apply_anthropic_settings(&mut request, settings);
        append_anthropic_tools(&mut request, tools);
        Ok(Value::Object(request))
    }

    /// Parse a provider wire response.
    ///
    /// # Errors
    ///
    /// Returns an error when required Anthropic response structure is malformed.
    pub fn parse_response(value: &Value) -> Result<ModelResponse, ModelError> {
        let mut parts = Vec::new();
        for block in value
            .get("content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if let Some(text) = block.get("text").and_then(Value::as_str) {
                        parts.push(ModelResponsePart::Text {
                            text: text.to_string(),
                        });
                    }
                }
                Some("thinking") => parts.push(ModelResponsePart::Thinking {
                    text: block
                        .get("thinking")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    signature: block
                        .get("signature")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                }),
                Some("tool_use") => parts.push(ModelResponsePart::ToolCall(ToolCallPart {
                    id: block
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    name: block
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    arguments: block.get("input").cloned().unwrap_or(Value::Null),
                })),
                _ => {}
            }
        }

        Ok(ModelResponse {
            parts,
            usage: usage_from_named(value, "input_tokens", "output_tokens"),
            model_name: value
                .get("model")
                .and_then(Value::as_str)
                .map(str::to_string),
            provider: Some(ProviderInfo {
                name: "anthropic".to_string(),
                response_id: value.get("id").and_then(Value::as_str).map(str::to_string),
            }),
            finish_reason: match value.get("stop_reason").and_then(Value::as_str) {
                Some("end_turn") => Some(FinishReason::Stop),
                Some("max_tokens") => Some(FinishReason::Length),
                Some("tool_use") => Some(FinishReason::ToolCalls),
                Some(_) => Some(FinishReason::Unknown),
                None => None,
            },
            timestamp: None,
            run_id: None,
            conversation_id: None,
            metadata: serde_json::Map::new(),
        })
    }
}

fn apply_anthropic_settings(
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
}

fn append_anthropic_tools(request: &mut serde_json::Map<String, Value>, tools: &[ToolDefinition]) {
    if tools.is_empty() {
        return;
    }
    request.insert(
        "tools".to_string(),
        json!(tools
            .iter()
            .map(|tool| {
                let mut definition = serde_json::Map::new();
                definition.insert("name".to_string(), json!(tool.name));
                insert_optional_description(&mut definition, tool.description.as_ref());
                definition.insert(
                    "input_schema".to_string(),
                    provider_tool_parameters(&tool.parameters),
                );
                Value::Object(definition)
            })
            .collect::<Vec<_>>()),
    );
}

fn anthropic_tool_result(tool_return: &ToolReturnPart) -> Value {
    let mut result = json!({
        "type": "tool_result",
        "tool_use_id": tool_return.tool_call_id,
        "content": tool_return.content.to_string(),
        "is_error": tool_return.is_error,
    });
    if let Some(cache_control) = tool_return.metadata.get("cache_control") {
        result["cache_control"] = cache_control.clone();
    }
    result
}
