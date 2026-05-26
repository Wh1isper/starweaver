//! `OpenAI` Responses wire mapper.

use serde_json::{json, Value};

use crate::{
    adapter::{NativeToolDefinition, ToolDefinition},
    message::{
        ModelMessage, ModelRequestPart, ModelResponse, ModelResponsePart, ProviderInfo,
        ToolCallPart,
    },
    providers::{
        apply_common_settings, finish_reason_openai, parse_tool_call_arguments, text_from_content,
        usage_from_openai,
    },
    ModelError, ModelSettings,
};

/// `OpenAI` Responses wire mapper.
pub struct OpenAiResponsesAdapter;

impl OpenAiResponsesAdapter {
    /// Build a provider wire request.
    ///
    /// # Errors
    ///
    /// Returns an error when canonical history cannot be mapped into response items.
    pub fn build_request(
        model: &str,
        messages: &[ModelMessage],
        settings: Option<&ModelSettings>,
        tools: &[ToolDefinition],
        native_tools: &[NativeToolDefinition],
    ) -> Result<Value, ModelError> {
        let mut input = Vec::new();
        let mut instructions = Vec::new();

        for message in messages {
            match message {
                ModelMessage::Request(request) => {
                    for part in &request.parts {
                        match part {
                            ModelRequestPart::SystemPrompt { text, .. }
                            | ModelRequestPart::Instruction { text, .. } => instructions.push(text.clone()),
                            ModelRequestPart::UserPrompt { content, .. } => input.push(json!({
                                "role": "user",
                                "content": [{"type": "input_text", "text": text_from_content(content)}]
                            })),
                            ModelRequestPart::ToolReturn(tool_return) => input.push(json!({
                                "type": "function_call_output",
                                "call_id": tool_return.tool_call_id,
                                "output": tool_return.content.to_string(),
                            })),
                            ModelRequestPart::RetryPrompt { text, .. } => input.push(json!({
                                "role": "user",
                                "content": [{"type": "input_text", "text": text}]
                            })),
                        }
                    }
                }
                ModelMessage::Response(response) => {
                    let text = response.text_output();
                    if !text.is_empty() {
                        input.push(json!({
                            "role": "assistant",
                            "content": [{"type": "output_text", "text": text}]
                        }));
                    }
                    for call in response.tool_calls() {
                        input.push(json!({
                            "type": "function_call",
                            "call_id": call.id,
                            "name": call.name,
                            "arguments": call.arguments.to_string(),
                        }));
                    }
                }
            }
        }

        let mut request = serde_json::Map::new();
        request.insert("model".to_string(), json!(model));
        request.insert("input".to_string(), json!(input));
        if !instructions.is_empty() {
            request.insert("instructions".to_string(), json!(instructions.join("\n\n")));
        }
        apply_common_settings(&mut request, settings);
        let tool_defs = response_tool_defs(tools, native_tools);
        if !tool_defs.is_empty() {
            request.insert("tools".to_string(), json!(tool_defs));
        }
        Ok(Value::Object(request))
    }

    /// Parse a provider wire response.
    ///
    /// # Errors
    ///
    /// Returns an error when required response item structure is malformed.
    pub fn parse_response(value: &Value) -> Result<ModelResponse, ModelError> {
        let mut parts = Vec::new();
        for item in value
            .get("output")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            match item.get("type").and_then(Value::as_str) {
                Some("message") => {
                    for content in item
                        .get("content")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                    {
                        if matches!(
                            content.get("type").and_then(Value::as_str),
                            Some("output_text")
                        ) {
                            if let Some(text) = content.get("text").and_then(Value::as_str) {
                                parts.push(ModelResponsePart::Text {
                                    text: text.to_string(),
                                });
                            }
                        }
                    }
                }
                Some("function_call") => parts.push(ModelResponsePart::ToolCall(ToolCallPart {
                    id: item
                        .get("call_id")
                        .or_else(|| item.get("id"))
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    name: item
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    arguments: parse_tool_call_arguments(
                        item.get("arguments").unwrap_or(&Value::Null),
                    ),
                })),
                _ => {}
            }
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
            finish_reason: value
                .get("status")
                .and_then(Value::as_str)
                .map(finish_reason_openai),
            timestamp: None,
            run_id: None,
            conversation_id: None,
            metadata: serde_json::Map::new(),
        })
    }
}

fn response_tool_defs(
    tools: &[ToolDefinition],
    native_tools: &[NativeToolDefinition],
) -> Vec<Value> {
    let mut definitions = tools
        .iter()
        .map(|tool| {
            json!({
                "type": "function",
                "name": tool.name,
                "description": tool.description,
                "parameters": tool.parameters,
            })
        })
        .collect::<Vec<_>>();
    definitions.extend(native_tools.iter().map(native_response_tool_def));
    definitions
}

fn native_response_tool_def(tool: &NativeToolDefinition) -> Value {
    let mut object = serde_json::Map::new();
    object.insert("type".to_string(), json!(tool.tool_type));
    for (key, value) in &tool.config {
        object.insert(key.clone(), value.clone());
    }
    Value::Object(object)
}
