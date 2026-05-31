//! AWS Bedrock Converse wire mapper.

use serde_json::{json, Value};

use crate::{
    adapter::ToolDefinition,
    message::{
        FinishReason, ModelMessage, ModelRequestPart, ModelResponse, ModelResponsePart,
        ProviderInfo, ToolCallPart,
    },
    providers::{
        bedrock_content_from_content, collect_system_and_non_system, insert_optional_description,
        provider_tool_parameters, usage_from_named,
    },
    ModelError, ModelSettings,
};

/// Bedrock Converse wire mapper.
pub struct BedrockConverseAdapter;

impl BedrockConverseAdapter {
    /// Build a provider wire request.
    ///
    /// # Errors
    ///
    /// Returns an error when canonical history cannot be mapped into Bedrock Converse messages.
    #[allow(clippy::too_many_lines)]
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
                                content.extend(bedrock_content_from_content(user_content));
                            }
                            ModelRequestPart::ToolReturn(tool_return) => content.push(json!({
                                "toolResult": {
                                    "toolUseId": tool_return.tool_call_id,
                                    "content": [{"json": tool_return.content}],
                                    "status": if tool_return.is_error { "error" } else { "success" }
                                }
                            })),
                            ModelRequestPart::RetryPrompt { text, .. } => {
                                content.push(json!({"text": text}));
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
                            ModelResponsePart::Text { text } => content.push(json!({"text": text})),
                            ModelResponsePart::ToolCall(call) => content.push(json!({
                                "toolUse": {
                                    "toolUseId": call.id,
                                    "name": call.name,
                                    "input": call.arguments,
                                }
                            })),
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
        request.insert("modelId".to_string(), json!(model));
        request.insert("messages".to_string(), json!(wire_messages));
        if !system.is_empty() {
            request.insert(
                "system".to_string(),
                json!(system
                    .into_iter()
                    .map(|text| json!({"text": text}))
                    .collect::<Vec<_>>()),
            );
        }
        if let Some(settings) = settings {
            let mut inference_config = serde_json::Map::new();
            if let Some(max_tokens) = settings.max_tokens {
                inference_config.insert("maxTokens".to_string(), json!(max_tokens));
            }
            if let Some(temperature) = settings.temperature {
                inference_config.insert("temperature".to_string(), json!(temperature));
            }
            if let Some(top_p) = settings.top_p {
                inference_config.insert("topP".to_string(), json!(top_p));
            }
            if !settings.stop_sequences.is_empty() {
                inference_config
                    .insert("stopSequences".to_string(), json!(settings.stop_sequences));
            }
            if !inference_config.is_empty() {
                request.insert(
                    "inferenceConfig".to_string(),
                    Value::Object(inference_config),
                );
            }
            if let Some(options) = settings
                .provider_options
                .as_ref()
                .and_then(Value::as_object)
            {
                if !options.is_empty() {
                    request.insert("additionalModelRequestFields".to_string(), json!(options));
                }
            }
            if let Some(fields) = settings.extra_body.get("additionalModelResponseFieldPaths") {
                request.insert(
                    "additionalModelResponseFieldPaths".to_string(),
                    fields.clone(),
                );
            }
        }
        if !tools.is_empty() {
            let mut tool_config = json!({
                "tools": tools
                    .iter()
                    .map(|tool| {
                        let mut spec = serde_json::Map::new();
                        spec.insert("name".to_string(), json!(tool.name));
                        insert_optional_description(&mut spec, tool.description.as_ref());
                        spec.insert(
                            "inputSchema".to_string(),
                            json!({"json": provider_tool_parameters(&tool.parameters)}),
                        );
                        json!({"toolSpec": spec})
                    })
                    .collect::<Vec<_>>()
            });
            if let Some(choice) = settings.and_then(|settings| settings.tool_choice.as_ref()) {
                tool_config["toolChoice"] = match choice {
                    crate::settings::ToolChoice::Auto | crate::settings::ToolChoice::None => {
                        json!({"auto": {}})
                    }
                    crate::settings::ToolChoice::Required => json!({"any": {}}),
                    crate::settings::ToolChoice::Tool { name } => json!({"tool": {"name": name}}),
                };
            }
            request.insert("toolConfig".to_string(), tool_config);
        }
        Ok(Value::Object(request))
    }

    /// Parse a provider wire response.
    ///
    /// # Errors
    ///
    /// Returns an error when required Bedrock response structure is malformed.
    pub fn parse_response(value: &Value) -> Result<ModelResponse, ModelError> {
        let mut parts = Vec::new();
        for item in value
            .get("output")
            .and_then(|output| output.get("message"))
            .and_then(|message| message.get("content"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(text) = item.get("text").and_then(Value::as_str) {
                parts.push(ModelResponsePart::Text {
                    text: text.to_string(),
                });
            }
            if let Some(call) = item.get("toolUse") {
                parts.push(ModelResponsePart::ToolCall(ToolCallPart {
                    id: call
                        .get("toolUseId")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    name: call
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    arguments: call.get("input").cloned().unwrap_or(Value::Null),
                }));
            }
        }

        Ok(ModelResponse {
            parts,
            usage: usage_from_named(value, "inputTokens", "outputTokens"),
            model_name: None,
            provider: Some(ProviderInfo {
                name: "bedrock".to_string(),
                response_id: value
                    .get("ResponseMetadata")
                    .and_then(|meta| meta.get("RequestId"))
                    .and_then(Value::as_str)
                    .map(str::to_string),
            }),
            finish_reason: match value.get("stopReason").and_then(Value::as_str) {
                Some("end_turn") => Some(FinishReason::Stop),
                Some("max_tokens") => Some(FinishReason::Length),
                Some("tool_use") => Some(FinishReason::ToolCalls),
                Some("guardrail_intervened" | "content_filtered") => {
                    Some(FinishReason::ContentFilter)
                }
                Some(_) => Some(FinishReason::Unknown),
                None => None,
            },
            timestamp: None,
            run_id: None,
            conversation_id: None,
            metadata: bedrock_metadata(value),
        })
    }
}

fn bedrock_metadata(value: &Value) -> serde_json::Map<String, Value> {
    let mut metadata = serde_json::Map::new();
    if let Some(fields) = value.get("additionalModelResponseFields") {
        metadata.insert(
            "additional_model_response_fields".to_string(),
            fields.clone(),
        );
    }
    if let Some(metrics) = value.get("metrics") {
        metadata.insert("metrics".to_string(), metrics.clone());
    }
    if value.get("metrics").is_some() {
        if let Some(response_metadata) = value.get("ResponseMetadata") {
            metadata.insert("response_metadata".to_string(), response_metadata.clone());
        }
    }
    metadata
}
