//! Gemini generateContent wire mapper.

use serde_json::{json, Value};

use crate::{
    adapter::{NativeToolDefinition, ToolDefinition},
    message::{
        FinishReason, ModelMessage, ModelRequestPart, ModelResponse, ModelResponsePart,
        ProviderInfo, ToolCallPart,
    },
    providers::{
        collect_system_and_non_system, gemini_parts_from_content, insert_optional_description,
        provider_tool_parameters, usage_from_named,
    },
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
        Self::build_request_with_native_tools(messages, settings, tools, &[])
    }

    /// Build a provider wire request including native Gemini tools.
    ///
    /// # Errors
    ///
    /// Returns an error when canonical history cannot be mapped into Gemini contents.
    pub fn build_request_with_native_tools(
        messages: &[ModelMessage],
        settings: Option<&ModelSettings>,
        tools: &[ToolDefinition],
        native_tools: &[NativeToolDefinition],
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
                                parts.extend(gemini_parts_from_content(content));
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
        append_gemini_generation_config(&mut request, settings);
        append_gemini_tools(&mut request, settings, tools, native_tools);
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
                Some("SAFETY" | "RECITATION" | "PROHIBITED_CONTENT") => {
                    Some(FinishReason::ContentFilter)
                }
                Some(_) => Some(FinishReason::Unknown),
                None => None,
            },
            timestamp: None,
            run_id: None,
            conversation_id: None,
            metadata: gemini_metadata(value, candidate),
        })
    }
}

fn append_gemini_generation_config(
    request: &mut serde_json::Map<String, Value>,
    settings: Option<&ModelSettings>,
) {
    let Some(settings) = settings else {
        return;
    };
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
        generation_config.insert("stopSequences".to_string(), json!(settings.stop_sequences));
    }
    if let Some(thinking) = &settings.thinking {
        let mut thinking_config = serde_json::Map::new();
        if let Some(budget_tokens) = thinking.budget_tokens {
            thinking_config.insert("thinkingBudget".to_string(), json!(budget_tokens));
        }
        if !thinking.effort.is_empty() {
            thinking_config.insert("thinkingLevel".to_string(), json!(thinking.effort));
        }
        if let Some(include_thoughts) = thinking.include_thoughts {
            thinking_config.insert("includeThoughts".to_string(), json!(include_thoughts));
        }
        generation_config.insert("thinkingConfig".to_string(), Value::Object(thinking_config));
    }
    if !generation_config.is_empty() {
        request.insert(
            "generationConfig".to_string(),
            Value::Object(generation_config),
        );
    }
}

fn append_gemini_tools(
    request: &mut serde_json::Map<String, Value>,
    settings: Option<&ModelSettings>,
    tools: &[ToolDefinition],
    native_tools: &[NativeToolDefinition],
) {
    let mut tool_defs = Vec::new();
    if !tools.is_empty() {
        tool_defs.push(json!({ "functionDeclarations": tools
            .iter()
            .map(|tool| {
                let mut declaration = serde_json::Map::new();
                declaration.insert("name".to_string(), json!(tool.name));
                insert_optional_description(&mut declaration, tool.description.as_ref());
                declaration.insert(
                    "parameters".to_string(),
                    provider_tool_parameters(&tool.parameters),
                );
                Value::Object(declaration)
            })
            .collect::<Vec<_>>() }));
        if let Some(choice) = settings.and_then(|settings| settings.tool_choice.as_ref()) {
            request.insert(
                "toolConfig".to_string(),
                json!({"functionCallingConfig": gemini_tool_choice(choice)}),
            );
        }
    }
    tool_defs.extend(native_tools.iter().map(gemini_native_tool));
    if !tool_defs.is_empty() {
        request.insert("tools".to_string(), Value::Array(tool_defs));
    }
}

fn gemini_tool_choice(choice: &crate::settings::ToolChoice) -> Value {
    match choice {
        crate::settings::ToolChoice::Auto => json!({"mode": "AUTO"}),
        crate::settings::ToolChoice::None => json!({"mode": "NONE"}),
        crate::settings::ToolChoice::Required => json!({"mode": "ANY"}),
        crate::settings::ToolChoice::Tool { name } => {
            json!({"mode": "ANY", "allowedFunctionNames": [name]})
        }
    }
}

fn gemini_metadata(value: &Value, candidate: &Value) -> serde_json::Map<String, Value> {
    let mut metadata = serde_json::Map::new();
    if let Some(ratings) = candidate.get("safetyRatings") {
        metadata.insert("safety_ratings".to_string(), ratings.clone());
    }
    if let Some(feedback) = value.get("promptFeedback") {
        metadata.insert("prompt_feedback".to_string(), feedback.clone());
    }
    metadata
}

fn gemini_native_tool(tool: &NativeToolDefinition) -> Value {
    match tool.tool_type.as_str() {
        "google_search" => json!({"googleSearch": tool.config}),
        "code_execution" => json!({"codeExecution": tool.config}),
        _ => {
            let mut object = serde_json::Map::new();
            object.insert(tool.tool_type.clone(), Value::Object(tool.config.clone()));
            Value::Object(object)
        }
    }
}
