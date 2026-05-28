//! `OpenAI` Chat Completions wire mapper.

use serde_json::{json, Value};

use crate::{
    adapter::ToolDefinition,
    message::{
        ModelMessage, ModelRequestPart, ModelResponse, ModelResponsePart, ProviderInfo,
        ToolCallPart,
    },
    providers::{
        apply_common_settings, finish_reason_openai, openai_chat_content,
        parse_tool_call_arguments, usage_from_openai,
    },
    ModelError, ModelSettings,
};

/// `OpenAI` Chat Completions wire mapper.
pub struct OpenAiChatAdapter;

impl OpenAiChatAdapter {
    /// Build a provider wire request.
    ///
    /// # Errors
    ///
    /// Returns an error when canonical history cannot be mapped into chat messages.
    pub fn build_request(
        model: &str,
        messages: &[ModelMessage],
        settings: Option<&ModelSettings>,
        tools: &[ToolDefinition],
    ) -> Result<Value, ModelError> {
        let mut wire_messages = Vec::new();
        for message in messages {
            match message {
                ModelMessage::Request(request) => {
                    for part in &request.parts {
                        match part {
                            ModelRequestPart::SystemPrompt { text, .. }
                            | ModelRequestPart::Instruction { text, .. } => {
                                wire_messages.push(json!({"role": "system", "content": text}));
                            }
                            ModelRequestPart::UserPrompt { content, .. } => {
                                wire_messages.push(
                                    json!({"role": "user", "content": openai_chat_content(content)}),
                                );
                            }
                            ModelRequestPart::ToolReturn(tool_return) => {
                                wire_messages.push(json!({
                                    "role": "tool",
                                    "tool_call_id": tool_return.tool_call_id,
                                    "content": tool_return.content.to_string(),
                                }));
                            }
                            ModelRequestPart::RetryPrompt { text, .. } => {
                                wire_messages.push(json!({"role": "user", "content": text}));
                            }
                        }
                    }
                }
                ModelMessage::Response(response) => {
                    let mut content = String::new();
                    let mut tool_calls = Vec::new();
                    for part in &response.parts {
                        match part {
                            ModelResponsePart::Text { text } => content.push_str(text),
                            ModelResponsePart::ToolCall(call) => tool_calls.push(json!({
                                "id": call.id,
                                "type": "function",
                                "function": {
                                    "name": call.name,
                                    "arguments": call.arguments.to_string(),
                                }
                            })),
                            _ => {}
                        }
                    }
                    let mut item = json!({"role": "assistant", "content": content});
                    if !tool_calls.is_empty() {
                        item["tool_calls"] = json!(tool_calls);
                    }
                    wire_messages.push(item);
                }
            }
        }

        let mut request = serde_json::Map::new();
        request.insert("model".to_string(), json!(model));
        request.insert("messages".to_string(), json!(wire_messages));
        apply_common_settings(&mut request, settings);
        if let Some(tool_choice) = settings.and_then(|settings| settings.tool_choice.as_ref()) {
            request.insert(
                "tool_choice".to_string(),
                crate::providers::openai_chat_tool_choice(tool_choice),
            );
        }
        if !tools.is_empty() {
            request.insert(
                "tools".to_string(),
                json!(tools
                    .iter()
                    .map(|tool| json!({
                        "type": "function",
                        "function": {
                            "name": tool.name,
                            "description": tool.description,
                            "parameters": tool.parameters,
                        }
                    }))
                    .collect::<Vec<_>>()),
            );
        }
        Ok(Value::Object(request))
    }

    /// Parse a provider wire response.
    ///
    /// # Errors
    ///
    /// Returns an error when the response is missing the first choice or message.
    pub fn parse_response(value: &Value) -> Result<ModelResponse, ModelError> {
        let choice = value
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
            .ok_or_else(|| ModelError::ResponseParsing("missing choices[0]".to_string()))?;
        let message = choice
            .get("message")
            .ok_or_else(|| ModelError::ResponseParsing("missing message".to_string()))?;

        let mut parts = Vec::new();
        if let Some(content) = message.get("content").and_then(Value::as_str) {
            if !content.is_empty() {
                parts.push(ModelResponsePart::Text {
                    text: content.to_string(),
                });
            }
        }
        if let Some(refusal) = message.get("refusal").and_then(Value::as_str) {
            if !refusal.is_empty() {
                parts.push(ModelResponsePart::Text {
                    text: refusal.to_string(),
                });
            }
        }
        for call in message
            .get("tool_calls")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let function = call.get("function").unwrap_or(&Value::Null);
            parts.push(ModelResponsePart::ToolCall(ToolCallPart {
                id: call
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                name: function
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                arguments: parse_tool_call_arguments(
                    function.get("arguments").unwrap_or(&Value::Null),
                ),
            }));
        }

        Ok(ModelResponse {
            parts,
            usage: usage_from_openai(value),
            model_name: value
                .get("model")
                .and_then(Value::as_str)
                .map(str::to_string),
            provider: Some(ProviderInfo {
                name: "openai".to_string(),
                response_id: value.get("id").and_then(Value::as_str).map(str::to_string),
            }),
            finish_reason: choice
                .get("finish_reason")
                .and_then(Value::as_str)
                .map(finish_reason_openai),
            timestamp: None,
            run_id: None,
            conversation_id: None,
            metadata: serde_json::Map::new(),
        })
    }
}
