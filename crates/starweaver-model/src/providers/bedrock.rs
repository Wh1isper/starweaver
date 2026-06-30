//! AWS Bedrock Converse wire mapper.

use serde_json::{json, Value};

use crate::{
    adapter::ToolDefinition,
    message::{
        FinishReason, ModelMessage, ModelRequestPart, ModelResponse, ModelResponsePart,
        ProviderInfo, ProviderPartInfo, ToolCallPart,
    },
    providers::{
        bedrock_content_from_content, collect_system_parts_and_non_system,
        insert_nonempty_description, provider_tool_schema_without_meta,
        usage_from_named_including_cache_input, SystemInstructionPart,
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
                            ModelResponsePart::Text { text }
                            | ModelResponsePart::ProviderText { text, .. } => {
                                content.push(json!({"text": text}));
                            }
                            ModelResponsePart::Thinking { text, .. } if !text.is_empty() => {
                                content.push(json!({
                                    "text": format!("<think>\n{text}\n</think>")
                                }));
                            }
                            ModelResponsePart::ProviderThinking {
                                text,
                                signature,
                                provider,
                            } => {
                                if provider.is_provider("bedrock") {
                                    content.push(bedrock_reasoning_content(
                                        text,
                                        signature.as_deref(),
                                    ));
                                } else if !text.is_empty() {
                                    content.push(json!({
                                        "text": format!("<think>\n{text}\n</think>")
                                    }));
                                }
                            }
                            ModelResponsePart::ToolCall(call)
                            | ModelResponsePart::ProviderToolCall { call, .. } => {
                                content.push(json!({
                                    "toolUse": {
                                        "toolUseId": call.id,
                                        "name": call.name,
                                        "input": call.arguments,
                                    }
                                }));
                            }
                            ModelResponsePart::ProviderOpaque {
                                item_type,
                                payload,
                                provider,
                            } if provider.is_provider("bedrock")
                                && bedrock_opaque_replay_item(item_type, payload) =>
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
        let model_id = settings
            .and_then(|settings| settings.provider_settings.bedrock.as_ref())
            .and_then(|bedrock| bedrock.inference_profile.as_deref())
            .unwrap_or(model);
        request.insert("modelId".to_string(), json!(model_id));
        request.insert("messages".to_string(), json!(wire_messages));
        if let Some(system) = bedrock_system_value(&system, settings) {
            request.insert("system".to_string(), system);
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
            append_bedrock_typed_fields(&mut request, model, settings);
            if let Some(fields) = settings.extra_body.get("additionalModelResponseFieldPaths") {
                request.insert(
                    "additionalModelResponseFieldPaths".to_string(),
                    fields.clone(),
                );
            }
        }
        let tools_disabled = matches!(
            settings.and_then(|settings| settings.tool_choice.as_ref()),
            Some(crate::settings::ToolChoice::None)
        );
        if !tools.is_empty() && !tools_disabled {
            let mut tool_definitions = tools
                .iter()
                .map(|tool| {
                    let mut spec = serde_json::Map::new();
                    spec.insert("name".to_string(), json!(tool.name));
                    insert_nonempty_description(&mut spec, tool.description.as_ref());
                    spec.insert(
                        "inputSchema".to_string(),
                        json!({"json": provider_tool_schema_without_meta(&tool.parameters)}),
                    );
                    json!({"toolSpec": spec})
                })
                .collect::<Vec<_>>();
            if let Some(cache_setting) =
                bedrock_cache_setting(settings, "bedrock_cache_tool_definitions")
            {
                tool_definitions.push(bedrock_cache_point(cache_setting));
            }
            let mut tool_config = json!({
                "tools": tool_definitions
            });
            if let Some(choice) = settings.and_then(|settings| settings.tool_choice.as_ref()) {
                tool_config["toolChoice"] = match choice {
                    crate::settings::ToolChoice::Auto | crate::settings::ToolChoice::None => {
                        json!({"auto": {}})
                    }
                    crate::settings::ToolChoice::Required
                    | crate::settings::ToolChoice::Tools { .. } => json!({"any": {}}),
                    crate::settings::ToolChoice::ToolOrOutput { .. } => json!({"auto": {}}),
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
            if let Some(reasoning) = item.get("reasoningContent") {
                parts.push(bedrock_reasoning_part(reasoning, item));
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
                    arguments: call.get("input").cloned().unwrap_or(Value::Null).into(),
                }));
            }
        }

        Ok(ModelResponse {
            parts,
            usage: usage_from_named_including_cache_input(value, "inputTokens", "outputTokens"),
            model_name: None,
            provider: Some(ProviderInfo {
                name: "bedrock".to_string(),
                response_id: value
                    .get("ResponseMetadata")
                    .and_then(|meta| meta.get("RequestId"))
                    .and_then(Value::as_str)
                    .map(str::to_string),
                details: serde_json::Map::new(),
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

fn bedrock_reasoning_content(text: &str, signature: Option<&str>) -> Value {
    let mut reasoning_text = serde_json::Map::new();
    reasoning_text.insert("text".to_string(), json!(text));
    if let Some(signature) = signature {
        reasoning_text.insert("signature".to_string(), json!(signature));
    }
    json!({"reasoningContent": {"reasoningText": reasoning_text}})
}

fn bedrock_reasoning_part(reasoning: &Value, content_block: &Value) -> ModelResponsePart {
    if let Some(reasoning_text) = reasoning.get("reasoningText") {
        let text = reasoning_text
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let signature = reasoning_text
            .get("signature")
            .and_then(Value::as_str)
            .map(str::to_string);
        if !text.is_empty() || signature.is_some() {
            return ModelResponsePart::ProviderThinking {
                text,
                signature,
                provider: ProviderPartInfo::new("bedrock"),
            };
        }
    }
    ModelResponsePart::ProviderOpaque {
        item_type: "reasoningContent".to_string(),
        payload: content_block.clone(),
        provider: ProviderPartInfo::new("bedrock"),
    }
}

fn bedrock_opaque_replay_item(item_type: &str, payload: &Value) -> bool {
    item_type == "reasoningContent" && payload.get("reasoningContent").is_some()
}

fn append_bedrock_typed_fields(
    request: &mut serde_json::Map<String, Value>,
    model: &str,
    settings: &ModelSettings,
) {
    let mut additional_model_request_fields = settings
        .provider_options
        .as_ref()
        .and_then(Value::as_object)
        .map(|options| {
            options
                .iter()
                .filter(|(key, _)| !is_internal_bedrock_option(key))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect::<serde_json::Map<_, _>>()
        })
        .unwrap_or_default();

    if let Some(top_k) = settings.top_k {
        if bedrock_uses_anthropic_passthrough(model) {
            additional_model_request_fields.insert("top_k".to_string(), json!(top_k));
        }
    }
    if let Some(thinking) = &settings.thinking {
        if bedrock_uses_anthropic_passthrough(model) {
            let mut payload = serde_json::Map::new();
            payload.insert(
                "type".to_string(),
                json!(thinking.mode.as_deref().unwrap_or("enabled")),
            );
            if let Some(budget_tokens) = thinking.budget_tokens {
                payload.insert("budget_tokens".to_string(), json!(budget_tokens));
            }
            additional_model_request_fields.insert("thinking".to_string(), Value::Object(payload));
        }
    }
    if let Some(bedrock) = &settings.provider_settings.bedrock {
        if let Some(fields) = &bedrock.additional_model_request_fields {
            if let Some(fields) = fields.as_object() {
                additional_model_request_fields.extend(fields.clone());
            }
        }
        if let Some(guardrail_config) = &bedrock.guardrail_config {
            request.insert("guardrailConfig".to_string(), guardrail_config.clone());
        }
        if let Some(performance_config) = &bedrock.performance_config {
            request.insert("performanceConfig".to_string(), performance_config.clone());
        }
        if let Some(request_metadata) = &bedrock.request_metadata {
            request.insert("requestMetadata".to_string(), request_metadata.clone());
        }
        if !bedrock.additional_model_response_field_paths.is_empty() {
            request.insert(
                "additionalModelResponseFieldPaths".to_string(),
                json!(bedrock.additional_model_response_field_paths),
            );
        }
        if let Some(prompt_variables) = &bedrock.prompt_variables {
            request.insert("promptVariables".to_string(), prompt_variables.clone());
        }
    }
    if let Some(service_tier) = bedrock_service_tier(settings.service_tier.as_ref()) {
        request.insert("serviceTier".to_string(), json!({"type": service_tier}));
    }
    if !additional_model_request_fields.is_empty() {
        request.insert(
            "additionalModelRequestFields".to_string(),
            Value::Object(additional_model_request_fields),
        );
    }
}

fn bedrock_uses_anthropic_passthrough(model: &str) -> bool {
    model.to_ascii_lowercase().contains("anthropic")
        || model.to_ascii_lowercase().contains("claude")
}

const fn bedrock_service_tier(
    service_tier: Option<&crate::settings::ServiceTier>,
) -> Option<&'static str> {
    match service_tier {
        Some(crate::settings::ServiceTier::Default) => Some("default"),
        Some(crate::settings::ServiceTier::Flex) => Some("flex"),
        Some(crate::settings::ServiceTier::Priority) => Some("priority"),
        Some(crate::settings::ServiceTier::Auto) | None => None,
    }
}

fn bedrock_system_value(
    system: &[SystemInstructionPart],
    settings: Option<&ModelSettings>,
) -> Option<Value> {
    if system.is_empty() {
        return None;
    }
    let mut blocks = system
        .iter()
        .map(|part| json!({"text": part.text}))
        .collect::<Vec<_>>();
    if let Some(cache_point) = bedrock_instruction_cache_point(system, settings) {
        blocks.insert(cache_point.index, cache_point.value);
    }
    Some(Value::Array(blocks))
}

struct BedrockCachePoint {
    index: usize,
    value: Value,
}

fn bedrock_instruction_cache_point(
    system: &[SystemInstructionPart],
    settings: Option<&ModelSettings>,
) -> Option<BedrockCachePoint> {
    let cache_setting = bedrock_cache_setting(settings, "bedrock_cache_instructions")?;
    let index = if let Some(first_dynamic) = system.iter().position(|part| part.dynamic) {
        let num_static = system[..first_dynamic]
            .iter()
            .filter(|part| !part.dynamic)
            .count();
        if num_static == 0 {
            return None;
        }
        first_dynamic
    } else {
        system.len()
    };
    Some(BedrockCachePoint {
        index,
        value: bedrock_cache_point(cache_setting),
    })
}

#[derive(Clone, Copy)]
enum BedrockCacheSetting {
    Default,
    Ttl(&'static str),
}

fn bedrock_cache_setting(
    settings: Option<&ModelSettings>,
    key: &str,
) -> Option<BedrockCacheSetting> {
    let value = settings
        .and_then(|settings| settings.provider_options.as_ref())
        .and_then(|options| options.get(key));
    match value {
        Some(Value::Bool(true)) => Some(BedrockCacheSetting::Default),
        Some(Value::String(value)) if value == "5m" => Some(BedrockCacheSetting::Ttl("5m")),
        Some(Value::String(value)) if value == "1h" => Some(BedrockCacheSetting::Ttl("1h")),
        _ => None,
    }
}

fn bedrock_cache_point(setting: BedrockCacheSetting) -> Value {
    let mut cache_point = serde_json::Map::from_iter([("type".to_string(), json!("default"))]);
    if let BedrockCacheSetting::Ttl(ttl) = setting {
        cache_point.insert("ttl".to_string(), json!(ttl));
    }
    json!({"cachePoint": cache_point})
}

fn is_internal_bedrock_option(key: &str) -> bool {
    matches!(
        key,
        "bedrock_cache_instructions" | "bedrock_cache_tool_definitions" | "bedrock_cache_messages"
    )
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
