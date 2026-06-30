//! Gemini generateContent wire mapper.

use serde_json::{json, Map, Value};

use crate::{
    adapter::{NativeToolDefinition, ToolDefinition},
    message::{
        FinishReason, ModelMessage, ModelRequest, ModelRequestPart, ModelResponse,
        ModelResponsePart, ProviderInfo, ProviderPartInfo, ToolCallPart,
    },
    providers::{
        collect_system_and_non_system, gemini_parts_from_content, insert_nonempty_description,
        provider_tool_schema_without_meta, usage_from_named_with_output_extras,
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
                    append_gemini_request_contents(&mut contents, request);
                }
                ModelMessage::Response(response) => {
                    append_gemini_response_content(&mut contents, response);
                }
            }
        }
        if contents
            .first()
            .is_none_or(|content| content.get("role").and_then(Value::as_str) == Some("model"))
        {
            contents.insert(0, json!({"role": "user", "parts": [{"text": ""}]}));
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
        append_gemini_typed_request_fields(&mut request, settings);
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
        let parts = gemini_response_parts(candidate);

        Ok(ModelResponse {
            parts,
            usage: usage_from_named_with_output_extras(
                value,
                "promptTokenCount",
                "candidatesTokenCount",
                &["thoughtsTokenCount", "thoughts_token_count"],
            ),
            model_name: None,
            provider: Some(ProviderInfo {
                name: "gemini".to_string(),
                response_id: None,
                details: serde_json::Map::new(),
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

fn gemini_response_parts(candidate: &Value) -> Vec<ModelResponsePart> {
    let mut parts = Vec::new();
    for part in candidate
        .get("content")
        .and_then(|content| content.get("parts"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        gemini_response_part(part, &mut parts);
    }
    parts
}

fn gemini_response_part(part: &Value, parts: &mut Vec<ModelResponsePart>) {
    let mut handled = false;
    if let Some(text) = part.get("text").and_then(Value::as_str) {
        handled = true;
        parts.push(gemini_text_response_part(text, part));
    }
    if let Some(call) = part.get("functionCall") {
        handled = true;
        parts.push(gemini_tool_call_response_part(call, part));
    }
    if !handled && (gemini_part_is_thought(part) || gemini_thought_signature(part).is_some()) {
        parts.push(ModelResponsePart::ProviderOpaque {
            item_type: "part".to_string(),
            payload: part.clone(),
            provider: ProviderPartInfo::new("gemini").with_details(gemini_part_details(part)),
        });
    }
}

fn gemini_text_response_part(text: &str, part: &Value) -> ModelResponsePart {
    let details = gemini_part_details(part);
    if gemini_part_is_thought(part) {
        ModelResponsePart::ProviderThinking {
            text: text.to_string(),
            signature: gemini_thought_signature(part).map(str::to_string),
            provider: ProviderPartInfo::new("gemini").with_details(details),
        }
    } else if details.is_empty() {
        ModelResponsePart::Text {
            text: text.to_string(),
        }
    } else {
        ModelResponsePart::ProviderText {
            text: text.to_string(),
            provider: ProviderPartInfo::new("gemini").with_details(details),
        }
    }
}

fn gemini_tool_call_response_part(call: &Value, part: &Value) -> ModelResponsePart {
    let call_id = gemini_call_id(call);
    let tool_call = ToolCallPart {
        id: call_id.clone().unwrap_or_else(|| {
            call.get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        }),
        name: call
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        arguments: call.get("args").cloned().unwrap_or(Value::Null).into(),
    };
    let details = gemini_part_details(part);
    if let Some(call_id) = call_id.filter(|_| !details.is_empty()) {
        ModelResponsePart::ProviderToolCall {
            call: tool_call,
            provider: ProviderPartInfo::new("gemini")
                .with_id(call_id)
                .with_details(details),
        }
    } else {
        ModelResponsePart::ToolCall(tool_call)
    }
}

fn append_gemini_request_contents(contents: &mut Vec<Value>, request: &ModelRequest) {
    let mut parts = Vec::new();
    for part in &request.parts {
        let mapped_parts = gemini_request_part(part);
        for mapped_part in mapped_parts {
            if gemini_should_split_user_parts(&parts, &mapped_part) {
                contents.push(json!({"role": "user", "parts": parts}));
                parts = Vec::new();
            }
            parts.push(mapped_part);
        }
    }
    if !parts.is_empty() {
        contents.push(json!({"role": "user", "parts": parts}));
    }
}

fn gemini_request_part(part: &ModelRequestPart) -> Vec<Value> {
    match part {
        ModelRequestPart::UserPrompt { content, .. } => gemini_parts_from_content(content),
        ModelRequestPart::ToolReturn(tool_return) => {
            let mut function_response = Map::new();
            insert_nonempty_value(&mut function_response, "id", &tool_return.tool_call_id);
            function_response.insert("name".to_string(), json!(tool_return.name));
            function_response.insert(
                "response".to_string(),
                json!({"content": tool_return.content}),
            );
            vec![json!({"functionResponse": function_response})]
        }
        ModelRequestPart::RetryPrompt { text, .. } => vec![json!({"text": text})],
        ModelRequestPart::SystemPrompt { .. } | ModelRequestPart::Instruction { .. } => Vec::new(),
    }
}

fn append_gemini_response_content(contents: &mut Vec<Value>, response: &ModelResponse) {
    let mut parts = Vec::new();
    let mut pending_legacy_thought_signature = None;
    let mut first_function_call_needs_signature = true;
    for part in &response.parts {
        let carried_signature = pending_legacy_thought_signature.take();
        append_gemini_response_part(
            &mut parts,
            part,
            carried_signature.as_deref(),
            &mut pending_legacy_thought_signature,
            &mut first_function_call_needs_signature,
        );
    }
    if !parts.is_empty() {
        contents.push(json!({"role": "model", "parts": parts}));
    }
}

fn append_gemini_response_part(
    parts: &mut Vec<Value>,
    part: &ModelResponsePart,
    carried_signature: Option<&str>,
    pending_legacy_thought_signature: &mut Option<String>,
    first_function_call_needs_signature: &mut bool,
) {
    match part {
        ModelResponsePart::ProviderText { text, provider } if provider.is_provider("gemini") => {
            parts.push(gemini_text_part(text, Some(provider), carried_signature));
        }
        ModelResponsePart::Text { text } | ModelResponsePart::ProviderText { text, .. } => {
            parts.push(json!({"text": text}));
        }
        ModelResponsePart::Thinking { text, .. } if !text.is_empty() => {
            parts.push(json!({"text": format!("<think>\n{text}\n</think>")}));
        }
        ModelResponsePart::ProviderThinking {
            text,
            signature,
            provider,
        } => {
            append_gemini_thinking_part(
                parts,
                text,
                signature.as_deref(),
                provider,
                carried_signature,
                pending_legacy_thought_signature,
            );
        }
        ModelResponsePart::ToolCall(call) => {
            parts.push(gemini_function_call_part(
                call,
                None,
                carried_signature,
                first_function_call_needs_signature,
            ));
        }
        ModelResponsePart::ProviderToolCall { call, provider } => {
            parts.push(gemini_function_call_part(
                call,
                Some(provider),
                carried_signature,
                first_function_call_needs_signature,
            ));
        }
        ModelResponsePart::ProviderOpaque {
            payload, provider, ..
        } if provider.is_provider("gemini") => {
            parts.push(payload.clone());
        }
        _ => {}
    }
}

fn append_gemini_thinking_part(
    parts: &mut Vec<Value>,
    text: &str,
    signature: Option<&str>,
    provider: &ProviderPartInfo,
    carried_signature: Option<&str>,
    pending_legacy_thought_signature: &mut Option<String>,
) {
    if provider.is_provider("gemini") {
        let inline_signature = gemini_provider_thought_signature(provider);
        if inline_signature.is_none() {
            *pending_legacy_thought_signature = signature.map(str::to_string);
        }
        if !text.is_empty() || inline_signature.is_some() {
            parts.push(gemini_thought_part(
                text,
                inline_signature.as_deref(),
                provider,
                carried_signature,
            ));
        }
    } else if !text.is_empty() {
        parts.push(json!({"text": format!("<think>\n{text}\n</think>")}));
    }
}

fn gemini_function_call_part(
    call: &ToolCallPart,
    provider: Option<&ProviderPartInfo>,
    carried_signature: Option<&str>,
    first_function_call_needs_signature: &mut bool,
) -> Value {
    let mut function_call = Map::new();
    insert_nonempty_value(&mut function_call, "id", &call.id);
    function_call.insert("name".to_string(), json!(call.name));
    function_call.insert("args".to_string(), json!(call.arguments));

    let mut part = Map::new();
    part.insert("functionCall".to_string(), Value::Object(function_call));
    if let Some(provider) = provider.filter(|provider| provider.is_provider("gemini")) {
        append_gemini_part_details(&mut part, provider);
    }
    if !part.contains_key("thoughtSignature") {
        if let Some(signature) = carried_signature {
            part.insert("thoughtSignature".to_string(), json!(signature));
        }
    }
    if *first_function_call_needs_signature {
        if !part.contains_key("thoughtSignature") {
            part.insert(
                "thoughtSignature".to_string(),
                json!("skip_thought_signature_validator"),
            );
        }
        *first_function_call_needs_signature = false;
    }
    Value::Object(part)
}

fn gemini_text_part(
    text: &str,
    provider: Option<&ProviderPartInfo>,
    carried_signature: Option<&str>,
) -> Value {
    let mut part = Map::new();
    part.insert("text".to_string(), json!(text));
    if let Some(provider) = provider {
        append_gemini_part_details(&mut part, provider);
    }
    if !part.contains_key("thoughtSignature") {
        if let Some(signature) = carried_signature {
            part.insert("thoughtSignature".to_string(), json!(signature));
        }
    }
    Value::Object(part)
}

fn gemini_thought_part(
    text: &str,
    inline_signature: Option<&str>,
    provider: &ProviderPartInfo,
    carried_signature: Option<&str>,
) -> Value {
    let mut part = Map::new();
    part.insert("text".to_string(), json!(text));
    part.insert("thought".to_string(), json!(true));
    append_gemini_part_details(&mut part, provider);
    if !part.contains_key("thoughtSignature") {
        if let Some(signature) = carried_signature.or(inline_signature) {
            part.insert("thoughtSignature".to_string(), json!(signature));
        }
    }
    Value::Object(part)
}

fn append_gemini_part_details(part: &mut Map<String, Value>, provider: &ProviderPartInfo) {
    if let Some(thought) = provider.details.get("thought").cloned() {
        part.insert("thought".to_string(), thought);
    }
    if let Some(signature) = provider
        .details
        .get("thoughtSignature")
        .or_else(|| provider.details.get("thought_signature"))
        .cloned()
    {
        part.insert("thoughtSignature".to_string(), signature);
    }
}

fn gemini_part_is_thought(part: &Value) -> bool {
    part.get("thought")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn gemini_thought_signature(part: &Value) -> Option<&str> {
    part.get("thoughtSignature")
        .or_else(|| part.get("thought_signature"))
        .and_then(Value::as_str)
}

fn gemini_provider_thought_signature(provider: &ProviderPartInfo) -> Option<String> {
    provider
        .details
        .get("thoughtSignature")
        .or_else(|| provider.details.get("thought_signature"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn gemini_part_details(part: &Value) -> Map<String, Value> {
    let mut details = Map::new();
    if let Some(thought) = part.get("thought").cloned() {
        details.insert("thought".to_string(), thought);
    }
    if let Some(signature) = part.get("thoughtSignature").cloned() {
        details.insert("thoughtSignature".to_string(), signature);
    } else if let Some(signature) = part.get("thought_signature").cloned() {
        details.insert("thoughtSignature".to_string(), signature);
    }
    details
}

fn gemini_call_id(call: &Value) -> Option<String> {
    call.get("id")
        .or_else(|| call.get("call_id"))
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

fn insert_nonempty_value(object: &mut Map<String, Value>, key: &str, value: &str) {
    if !value.is_empty() {
        object.insert(key.to_string(), json!(value));
    }
}

fn gemini_should_split_user_parts(existing_parts: &[Value], next_part: &Value) -> bool {
    existing_parts
        .last()
        .is_some_and(gemini_part_is_function_response)
        && !gemini_part_is_function_response(next_part)
}

fn gemini_part_is_function_response(part: &Value) -> bool {
    part.get("functionResponse").is_some()
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
    if let Some(seed) = settings.seed {
        generation_config.insert("seed".to_string(), json!(seed));
    }
    if let Some(presence_penalty) = settings.presence_penalty {
        generation_config.insert("presencePenalty".to_string(), json!(presence_penalty));
    }
    if let Some(frequency_penalty) = settings.frequency_penalty {
        generation_config.insert("frequencyPenalty".to_string(), json!(frequency_penalty));
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
        if let Some(mode) = &thinking.mode {
            thinking_config.insert("mode".to_string(), json!(mode));
        }
        if let Some(include_thoughts) = thinking.include_thoughts {
            thinking_config.insert("includeThoughts".to_string(), json!(include_thoughts));
        }
        generation_config.insert("thinkingConfig".to_string(), Value::Object(thinking_config));
    }
    if let Some(google) = &settings.provider_settings.google {
        if let Some(response_logprobs) = google.response_logprobs {
            generation_config.insert("responseLogprobs".to_string(), json!(response_logprobs));
        }
        if let Some(logprobs) = google.logprobs {
            generation_config.insert("logprobs".to_string(), json!(logprobs));
        }
    }
    if let Some(options) = settings
        .provider_options
        .as_ref()
        .and_then(Value::as_object)
    {
        generation_config.extend(options.iter().filter_map(|(key, value)| {
            key.strip_prefix("google_generation_config.")
                .map(|target| (target.to_string(), value.clone()))
        }));
    }
    if !generation_config.is_empty() {
        request.insert(
            "generationConfig".to_string(),
            Value::Object(generation_config),
        );
    }
}

fn append_gemini_typed_request_fields(
    request: &mut serde_json::Map<String, Value>,
    settings: Option<&ModelSettings>,
) {
    let Some(google) = settings.and_then(|settings| settings.provider_settings.google.as_ref())
    else {
        return;
    };
    if let Some(safety_settings) = &google.safety_settings {
        request.insert("safetySettings".to_string(), safety_settings.clone());
    }
    if let Some(cached_content) = &google.cached_content {
        request.insert("cachedContent".to_string(), json!(cached_content));
    }
    if let Some(labels) = &google.labels {
        request.insert("labels".to_string(), labels.clone());
    }
    if let Some(service_tier) = &google.service_tier {
        request.insert("serviceTier".to_string(), json!(service_tier));
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
                insert_nonempty_description(&mut declaration, tool.description.as_ref());
                declaration.insert(
                    "parameters".to_string(),
                    provider_tool_schema_without_meta(&tool.parameters),
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
        crate::settings::ToolChoice::Tools { names } => {
            json!({"mode": "ANY", "allowedFunctionNames": names})
        }
        crate::settings::ToolChoice::ToolOrOutput { function_tools } => {
            json!({"mode": "AUTO", "allowedFunctionNames": function_tools})
        }
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
