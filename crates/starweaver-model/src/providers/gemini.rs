//! Gemini generateContent wire mapper.

use serde_json::{json, Value};

use crate::{
    adapter::ToolDefinition,
    message::{
        FinishReason, ModelMessage, ModelRequestPart, ModelResponse, ModelResponsePart,
        ProviderInfo, ToolCallPart,
    },
    providers::{collect_system_and_non_system, text_from_content, usage_from_named},
    ModelError, ModelSettings,
};

/// Gemini generateContent wire mapper.
pub struct GeminiGenerateContentAdapter;

impl GeminiGenerateContentAdapter {
    /// Build a provider wire request.
    ///
    /// # Errors
    ///
    /// Returns an error when canonical history cannot be mapped into Gemini contents.
    pub fn build_request(
        messages: &[ModelMessage],
        settings: Option<&ModelSettings>,
        tools: &[ToolDefinition],
    ) -> Result<Value, ModelError> {
        let (system, rest) = collect_system_and_non_system(messages);
        let mut contents = Vec::new();

        for message in rest {
            match message {
                ModelMessage::Request(request) => {
                    let mut parts = Vec::new();
                    for part in &request.parts {
                        match part {
                            ModelRequestPart::UserPrompt { content, .. } => {
                                parts.push(json!({"text": text_from_content(content)}));
                            }
                            ModelRequestPart::ToolReturn(tool_return) => parts.push(json!({
                                "functionResponse": {
                                    "name": tool_return.name,
                                    "response": {"content": tool_return.content}
                                }
                            })),
                            ModelRequestPart::RetryPrompt { text, .. } => {
                                parts.push(json!({"text": text}));
                            }
                            ModelRequestPart::SystemPrompt { .. }
                            | ModelRequestPart::Instruction { .. } => {}
                        }
                    }
                    if !parts.is_empty() {
                        contents.push(json!({"role": "user", "parts": parts}));
                    }
                }
                ModelMessage::Response(response) => {
                    let mut parts = Vec::new();
                    for part in &response.parts {
                        match part {
                            ModelResponsePart::Text { text } => parts.push(json!({"text": text})),
                            ModelResponsePart::ToolCall(call) => parts.push(json!({
                                "functionCall": {
                                    "name": call.name,
                                    "args": call.arguments,
                                }
                            })),
                            _ => {}
                        }
                    }
                    if !parts.is_empty() {
                        contents.push(json!({"role": "model", "parts": parts}));
                    }
                }
            }
        }

        let mut request = serde_json::Map::new();
        request.insert("contents".to_string(), json!(contents));
        if !system.is_empty() {
            request.insert(
                "systemInstruction".to_string(),
                json!({"parts": [{"text": system.join("\n\n")}] }),
            );
        }
        if let Some(settings) = settings {
            let mut generation_config = serde_json::Map::new();
            if let Some(max_tokens) = settings.max_tokens {
                generation_config.insert("maxOutputTokens".to_string(), json!(max_tokens));
            }
            if let Some(temperature) = settings.temperature {
                generation_config.insert("temperature".to_string(), json!(temperature));
            }
            if let Some(top_p) = settings.top_p {
                generation_config.insert("topP".to_string(), json!(top_p));
            }
            if let Some(top_k) = settings.top_k {
                generation_config.insert("topK".to_string(), json!(top_k));
            }
            if !settings.stop_sequences.is_empty() {
                generation_config
                    .insert("stopSequences".to_string(), json!(settings.stop_sequences));
            }
            if !generation_config.is_empty() {
                request.insert(
                    "generationConfig".to_string(),
                    Value::Object(generation_config),
                );
            }
        }
        if !tools.is_empty() {
            request.insert(
                "tools".to_string(),
                json!([{ "functionDeclarations": tools
                    .iter()
                    .map(|tool| json!({
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool.parameters,
                    }))
                    .collect::<Vec<_>>() }]),
            );
        }
        Ok(Value::Object(request))
    }

    /// Parse a provider wire response.
    ///
    /// # Errors
    ///
    /// Returns an error when the response is missing the first candidate.
    pub fn parse_response(value: &Value) -> Result<ModelResponse, ModelError> {
        let candidate = value
            .get("candidates")
            .and_then(Value::as_array)
            .and_then(|candidates| candidates.first())
            .ok_or_else(|| ModelError::ResponseParsing("missing candidates[0]".to_string()))?;
        let mut parts = Vec::new();
        for part in candidate
            .get("content")
            .and_then(|content| content.get("parts"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(text) = part.get("text").and_then(Value::as_str) {
                parts.push(ModelResponsePart::Text {
                    text: text.to_string(),
                });
            }
            if let Some(call) = part.get("functionCall") {
                parts.push(ModelResponsePart::ToolCall(ToolCallPart {
                    id: call
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or_else(|| {
                            call.get("name").and_then(Value::as_str).unwrap_or_default()
                        })
                        .to_string(),
                    name: call
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    arguments: call.get("args").cloned().unwrap_or(Value::Null),
                }));
            }
        }

        Ok(ModelResponse {
            parts,
            usage: usage_from_named(value, "promptTokenCount", "candidatesTokenCount"),
            model_name: None,
            provider: Some(ProviderInfo {
                name: "gemini".to_string(),
                response_id: None,
            }),
            finish_reason: match candidate.get("finishReason").and_then(Value::as_str) {
                Some("STOP") => Some(FinishReason::Stop),
                Some("MAX_TOKENS") => Some(FinishReason::Length),
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
