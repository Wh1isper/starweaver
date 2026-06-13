//! `OpenAI` Responses wire mapper.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};
use starweaver_core::{ConversationId, Usage};

use crate::{
    adapter::{NativeToolDefinition, ToolDefinition},
    message::{
        Metadata, ModelMessage, ModelRequestPart, ModelResponse, ModelResponsePart, ProviderInfo,
        ProviderPartInfo, ToolCallPart,
    },
    providers::{
        apply_common_settings_with_max_tokens, finish_reason_openai, insert_optional_description,
        is_dynamic_system_instruction, openai_responses_content, parse_tool_call_arguments,
        provider_tool_parameters, usage_from_openai,
    },
    transport::MaxTokensParameter,
    ModelError, ModelResponseStreamEvent, ModelSettings,
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
        Self::build_request_with_options(
            model,
            messages,
            settings,
            tools,
            native_tools,
            MaxTokensParameter::Default,
        )
    }

    /// Build a provider wire request with explicit gateway/provider options.
    ///
    /// # Errors
    ///
    /// Returns an error when canonical history cannot be mapped into response items.
    pub fn build_request_with_options(
        model: &str,
        messages: &[ModelMessage],
        settings: Option<&ModelSettings>,
        tools: &[ToolDefinition],
        native_tools: &[NativeToolDefinition],
        max_tokens_parameter: MaxTokensParameter,
    ) -> Result<Value, ModelError> {
        let replay = OpenAiReplayOptions::from_settings(settings);
        let instructions = collect_static_openai_instructions(messages);
        let (previous_response_id, conversation_id, messages) =
            resolve_server_side_state(messages, &replay)?;
        let mut input = Vec::new();

        for message in &messages {
            match message {
                ModelMessage::Request(request) => {
                    for part in &request.parts {
                        match part {
                            ModelRequestPart::SystemPrompt { .. } => {}
                            ModelRequestPart::Instruction { text, metadata } => {
                                if is_dynamic_instruction(metadata) {
                                    push_instruction_message(text, &mut input);
                                }
                            }
                            ModelRequestPart::UserPrompt { content, .. } => input.push(json!({
                                "role": "user",
                                "content": openai_responses_content(content)
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
                    push_response_replay_items(response, &replay, &mut input);
                }
            }
        }

        let mut request = serde_json::Map::new();
        if input.is_empty() && previous_response_id.is_none() && conversation_id.is_none() {
            input.push(json!({"role": "user", "content": ""}));
        }
        request.insert("model".to_string(), json!(model));
        request.insert("input".to_string(), json!(input));
        if !instructions.is_empty() {
            request.insert("instructions".to_string(), json!(instructions.join("\n\n")));
        }
        apply_common_settings_with_max_tokens(&mut request, settings, max_tokens_parameter);
        strip_openai_replay_aliases(&mut request);
        if let Some(previous_response_id) = previous_response_id {
            request.insert(
                "previous_response_id".to_string(),
                json!(previous_response_id),
            );
        }
        if let Some(conversation_id) = conversation_id {
            request.insert("conversation".to_string(), json!(conversation_id));
        }
        if replay.include_encrypted_reasoning {
            ensure_include(&mut request, "reasoning.encrypted_content");
        }
        if let Some(thinking) = settings.and_then(|settings| settings.thinking.as_ref()) {
            let mut reasoning = serde_json::Map::new();
            reasoning.insert("effort".to_string(), json!(thinking.effort));
            if let Some(summary) = &thinking.summary {
                reasoning.insert("summary".to_string(), json!(summary));
            }
            request.insert("reasoning".to_string(), Value::Object(reasoning));
            request.remove("reasoning_effort");
        }
        if let Some(tool_choice) = settings.and_then(|settings| settings.tool_choice.as_ref()) {
            request.insert(
                "tool_choice".to_string(),
                crate::providers::openai_responses_tool_choice(tool_choice),
            );
        }
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
            parse_response_item(item, &mut parts);
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
                details: openai_response_details(value),
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

    /// Parse `OpenAI` Responses server-sent JSON events into canonical stream events.
    ///
    /// # Errors
    ///
    /// Returns an error when no completed response is present in the event list.
    pub fn parse_stream_events(
        events: &[Value],
    ) -> Result<Vec<ModelResponseStreamEvent>, ModelError> {
        let mut parser = OpenAiResponsesStreamParser::default();
        let mut stream = Vec::new();
        for event in events {
            stream.extend(parser.push_event(event)?);
        }
        stream.extend(parser.finish()?);
        if stream
            .iter()
            .any(|event| matches!(event, ModelResponseStreamEvent::FinalResult(_)))
        {
            Ok(stream)
        } else {
            Err(ModelError::ResponseParsing(
                "missing response.completed event".to_string(),
            ))
        }
    }
}

#[derive(Clone, Debug)]
struct OpenAiReplayOptions {
    previous_response_id: Option<String>,
    conversation_id: Option<String>,
    send_item_ids: bool,
    include_encrypted_reasoning: bool,
}

impl OpenAiReplayOptions {
    fn from_settings(settings: Option<&ModelSettings>) -> Self {
        let provider_replay = settings.and_then(|settings| settings.provider_replay.as_ref());
        let previous_response_id = provider_replay
            .and_then(|replay| replay.previous_response_id.clone())
            .or_else(|| provider_setting_string(settings, &["openai_previous_response_id"]));
        let conversation_id = provider_replay
            .and_then(|replay| replay.conversation_id.clone())
            .or_else(|| provider_setting_string(settings, &["openai_conversation_id"]));
        let send_item_ids = provider_replay
            .and_then(|replay| replay.send_item_ids)
            .or_else(|| provider_setting_bool(settings, &["openai_send_reasoning_ids"]))
            .unwrap_or(true);
        let include_encrypted_reasoning = provider_replay
            .and_then(|replay| replay.include_encrypted_reasoning)
            .or_else(|| provider_setting_bool(settings, &["openai_include_encrypted_reasoning"]))
            .unwrap_or_else(|| {
                send_item_ids
                    && settings
                        .and_then(|settings| settings.thinking.as_ref())
                        .is_some()
            });
        Self {
            previous_response_id,
            conversation_id,
            send_item_ids,
            include_encrypted_reasoning,
        }
    }
}

fn provider_setting_string(settings: Option<&ModelSettings>, keys: &[&str]) -> Option<String> {
    let settings = settings?;
    keys.iter()
        .find_map(|key| setting_value(settings, key).and_then(Value::as_str))
        .map(str::to_string)
}

fn provider_setting_bool(settings: Option<&ModelSettings>, keys: &[&str]) -> Option<bool> {
    let settings = settings?;
    keys.iter()
        .find_map(|key| setting_value(settings, key).and_then(Value::as_bool))
}

fn setting_value<'a>(settings: &'a ModelSettings, key: &str) -> Option<&'a Value> {
    settings
        .provider_options
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|object| object.get(key))
        .or_else(|| settings.extra_body.get(key))
}

fn collect_static_openai_instructions(messages: &[ModelMessage]) -> Vec<String> {
    let mut instructions = Vec::new();
    for message in messages {
        let ModelMessage::Request(request) = message else {
            continue;
        };
        if let Some(request_instructions) = request.instructions.as_ref() {
            push_unique_instruction(&mut instructions, request_instructions);
        }
        for part in &request.parts {
            match part {
                ModelRequestPart::SystemPrompt { text, .. } => {
                    push_unique_instruction(&mut instructions, text);
                }
                ModelRequestPart::Instruction { text, metadata }
                    if !is_dynamic_instruction(metadata) =>
                {
                    push_unique_instruction(&mut instructions, text);
                }
                ModelRequestPart::Instruction { .. }
                | ModelRequestPart::UserPrompt { .. }
                | ModelRequestPart::ToolReturn(_)
                | ModelRequestPart::RetryPrompt { .. } => {}
            }
        }
    }
    instructions
}

fn push_unique_instruction(instructions: &mut Vec<String>, text: &str) {
    if text.trim().is_empty() || instructions.iter().any(|existing| existing == text) {
        return;
    }
    instructions.push(text.to_string());
}

fn is_dynamic_instruction(metadata: &Metadata) -> bool {
    is_dynamic_system_instruction(metadata)
}

fn push_instruction_message(text: &str, input: &mut Vec<Value>) {
    if text.trim().is_empty() {
        return;
    }
    input.push(json!({
        "role": "system",
        "content": [{"type": "input_text", "text": text}],
    }));
}

type ServerSideStateMessages<'a> = (Option<String>, Option<String>, Vec<&'a ModelMessage>);

fn resolve_server_side_state<'a>(
    messages: &'a [ModelMessage],
    replay: &OpenAiReplayOptions,
) -> Result<ServerSideStateMessages<'a>, ModelError> {
    if replay.previous_response_id.is_some() && replay.conversation_id.is_some() {
        return Err(ModelError::MessageMapping(
            "OpenAI Responses previous_response_id and conversation cannot both be set".to_string(),
        ));
    }
    if let Some(setting) = replay.conversation_id.as_deref() {
        let (conversation_id, trimmed) = resolve_conversation_id(messages, setting);
        return Ok((None, conversation_id, trimmed));
    }
    if let Some(setting) = replay.previous_response_id.as_deref() {
        let (previous_response_id, trimmed) = resolve_previous_response_id(messages, setting);
        return Ok((previous_response_id, None, trimmed));
    }
    Ok((None, None, messages.iter().collect()))
}

fn resolve_previous_response_id<'a>(
    messages: &'a [ModelMessage],
    setting: &str,
) -> (Option<String>, Vec<&'a ModelMessage>) {
    let mut trimmed = Vec::new();
    for message in messages.iter().rev() {
        if let ModelMessage::Response(response) = message {
            if is_openai_response(response) {
                if is_compaction_boundary(response) {
                    return (None, messages.iter().collect());
                }
                if let Some(response_id) = response
                    .provider
                    .as_ref()
                    .and_then(|p| p.response_id.clone())
                {
                    if !trimmed.is_empty() {
                        trimmed.reverse();
                        return (Some(response_id), trimmed);
                    }
                }
                break;
            }
        }
        trimmed.push(message);
    }
    if setting == "auto" || is_at_compaction_boundary(messages) {
        (None, messages.iter().collect())
    } else {
        (Some(setting.to_string()), messages.iter().collect())
    }
}

fn resolve_conversation_id<'a>(
    messages: &'a [ModelMessage],
    setting: &str,
) -> (Option<String>, Vec<&'a ModelMessage>) {
    if setting == "auto" {
        let active_conversation_id = messages.last().and_then(message_conversation_id);
        return get_conversation_id_and_new_messages(messages, None, active_conversation_id);
    }

    let (conversation_id, trimmed) =
        get_conversation_id_and_new_messages(messages, Some(setting), None);
    if conversation_id.is_some() {
        (conversation_id, trimmed)
    } else {
        (Some(setting.to_string()), messages.iter().collect())
    }
}

fn get_conversation_id_and_new_messages<'a>(
    messages: &'a [ModelMessage],
    openai_conversation_id: Option<&str>,
    active_conversation_id: Option<&str>,
) -> (Option<String>, Vec<&'a ModelMessage>) {
    let mut trimmed = Vec::new();
    for message in messages.iter().rev() {
        if let ModelMessage::Response(response) = message {
            if is_openai_response(response) {
                if active_conversation_id.is_some()
                    && response.conversation_id.is_some()
                    && response
                        .conversation_id
                        .as_ref()
                        .map(ConversationId::as_str)
                        != active_conversation_id
                {
                    trimmed.push(message);
                    continue;
                }
                if let Some(conversation_id) = response
                    .provider
                    .as_ref()
                    .and_then(|provider| provider.details.get("conversation_id"))
                    .and_then(Value::as_str)
                    .filter(|candidate| {
                        openai_conversation_id.map_or(true, |expected| expected == *candidate)
                    })
                {
                    trimmed.reverse();
                    return (Some(conversation_id.to_string()), trimmed);
                }
            }
        }
        trimmed.push(message);
    }
    (None, messages.iter().collect())
}

fn message_conversation_id(message: &ModelMessage) -> Option<&str> {
    match message {
        ModelMessage::Request(request) => {
            request.conversation_id.as_ref().map(ConversationId::as_str)
        }
        ModelMessage::Response(response) => response
            .conversation_id
            .as_ref()
            .map(ConversationId::as_str),
    }
}

fn is_openai_response(response: &ModelResponse) -> bool {
    response
        .provider
        .as_ref()
        .is_some_and(|provider| provider.name == "openai")
}

fn is_at_compaction_boundary(messages: &[ModelMessage]) -> bool {
    messages
        .iter()
        .rev()
        .find_map(|message| match message {
            ModelMessage::Response(response) if is_openai_response(response) => {
                Some(is_compaction_boundary(response))
            }
            ModelMessage::Request(_) | ModelMessage::Response(_) => None,
        })
        .unwrap_or(false)
}

fn is_compaction_boundary(response: &ModelResponse) -> bool {
    response.provider.as_ref().is_some_and(|provider| {
        provider
            .details
            .get("compaction")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }) || response.parts.iter().any(ModelResponsePart::is_compaction)
}

fn push_response_replay_items(
    response: &ModelResponse,
    replay: &OpenAiReplayOptions,
    input: &mut Vec<Value>,
) {
    for part in &response.parts {
        match part {
            ModelResponsePart::Text { text } => push_assistant_text(text, input),
            ModelResponsePart::ProviderText { text, provider } => {
                push_provider_text(text, provider, replay, input);
            }
            ModelResponsePart::Thinking { text, .. } => push_tagged_thinking(text, input),
            ModelResponsePart::ProviderThinking {
                text,
                signature,
                provider,
            } => push_provider_thinking(text, signature.as_deref(), provider, replay, input),
            ModelResponsePart::ToolCall(call) => push_function_call(call, None, replay, input),
            ModelResponsePart::ProviderToolCall { call, provider } => {
                push_function_call(call, Some(provider), replay, input);
            }
            ModelResponsePart::NativeToolCall { payload, .. } => {
                if replay.send_item_ids {
                    push_native_replay_payload(payload, input);
                }
            }
            ModelResponsePart::ProviderOpaque {
                payload, provider, ..
            } => {
                if replay.send_item_ids && provider.is_provider("openai") && provider.id.is_some() {
                    push_native_replay_payload(payload, input);
                }
            }
            ModelResponsePart::NativeToolReturn { .. }
            | ModelResponsePart::File { .. }
            | ModelResponsePart::Compaction { .. } => {}
        }
    }
}

fn push_assistant_text(text: &str, input: &mut Vec<Value>) {
    if text.is_empty() {
        return;
    }
    input.push(json!({
        "role": "assistant",
        "content": [{"type": "output_text", "text": text}]
    }));
}

fn push_provider_text(
    text: &str,
    provider: &ProviderPartInfo,
    replay: &OpenAiReplayOptions,
    input: &mut Vec<Value>,
) {
    if text.is_empty() {
        return;
    }
    let Some(id) = provider.id.as_deref() else {
        push_assistant_text(text, input);
        return;
    };
    if !(replay.send_item_ids && provider.is_provider("openai")) {
        push_assistant_text(text, input);
        return;
    }

    let content = output_text_replay_content(text, provider);
    if let Some(message) = find_openai_item_mut(input, "message", id) {
        append_array_field(message, "content", content);
        return;
    }

    let mut message = Map::new();
    message.insert("type".to_string(), json!("message"));
    message.insert("role".to_string(), json!("assistant"));
    message.insert("status".to_string(), json!("completed"));
    message.insert("id".to_string(), json!(id));
    if let Some(phase) = provider.details.get("phase").cloned() {
        message.insert("phase".to_string(), phase);
    }
    message.insert("content".to_string(), Value::Array(vec![content]));
    input.push(Value::Object(message));
}

fn output_text_replay_content(text: &str, provider: &ProviderPartInfo) -> Value {
    let mut content = Map::new();
    content.insert("type".to_string(), json!("output_text"));
    content.insert("text".to_string(), json!(text));
    content.insert(
        "annotations".to_string(),
        provider
            .details
            .get("annotations")
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new())),
    );
    Value::Object(content)
}

fn push_tagged_thinking(text: &str, input: &mut Vec<Value>) {
    if text.is_empty() {
        return;
    }
    input.push(json!({
        "role": "assistant",
        "content": [{"type": "output_text", "text": format!("<think>\n{text}\n</think>")}]
    }));
}

fn push_provider_thinking(
    text: &str,
    signature: Option<&str>,
    provider: &ProviderPartInfo,
    replay: &OpenAiReplayOptions,
    input: &mut Vec<Value>,
) {
    let raw_content = raw_reasoning_replay_content(provider);
    let Some(id) = provider.id.as_deref() else {
        push_tagged_thinking(text, input);
        return;
    };
    if !provider.is_provider("openai") || !replay.send_item_ids {
        push_tagged_thinking(text, input);
        return;
    }
    let encrypted_content = replay
        .include_encrypted_reasoning
        .then(|| {
            signature.or_else(|| {
                provider
                    .details
                    .get("encrypted_content")
                    .and_then(Value::as_str)
            })
        })
        .flatten();
    if encrypted_content.is_none() && text.is_empty() && raw_content.is_empty() {
        return;
    }

    if let Some(reasoning) = find_openai_item_mut(input, "reasoning", id) {
        update_reasoning_replay_item(reasoning, text, encrypted_content, &raw_content);
        return;
    }

    let mut reasoning = Map::new();
    reasoning.insert("type".to_string(), json!("reasoning"));
    reasoning.insert("id".to_string(), json!(id));
    reasoning.insert("summary".to_string(), Value::Array(Vec::new()));
    update_reasoning_replay_item(&mut reasoning, text, encrypted_content, &raw_content);
    input.push(Value::Object(reasoning));
}

fn raw_reasoning_replay_content(provider: &ProviderPartInfo) -> Vec<String> {
    provider
        .details
        .get("raw_content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

fn update_reasoning_replay_item(
    reasoning: &mut Map<String, Value>,
    text: &str,
    encrypted_content: Option<&str>,
    raw_content: &[String],
) {
    if let Some(encrypted_content) = encrypted_content {
        reasoning.insert("encrypted_content".to_string(), json!(encrypted_content));
    }
    if !text.is_empty() {
        append_array_field(
            reasoning,
            "summary",
            json!({"type": "summary_text", "text": text}),
        );
    }
    for text in raw_content {
        append_array_field(
            reasoning,
            "content",
            json!({"type": "reasoning_text", "text": text}),
        );
    }
}

fn find_openai_item_mut<'a>(
    input: &'a mut [Value],
    item_type: &str,
    id: &str,
) -> Option<&'a mut Map<String, Value>> {
    input.iter_mut().find_map(|item| {
        let object = item.as_object_mut()?;
        let same_type = object.get("type").and_then(Value::as_str) == Some(item_type);
        let same_id = object.get("id").and_then(Value::as_str) == Some(id);
        (same_type && same_id).then_some(object)
    })
}

fn append_array_field(object: &mut Map<String, Value>, key: &str, value: Value) {
    let entry = object
        .entry(key.to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    if let Some(items) = entry.as_array_mut() {
        items.push(value);
    }
}

fn push_function_call(
    call: &ToolCallPart,
    provider: Option<&ProviderPartInfo>,
    replay: &OpenAiReplayOptions,
    input: &mut Vec<Value>,
) {
    let mut item = Map::new();
    item.insert("type".to_string(), json!("function_call"));
    item.insert("call_id".to_string(), json!(call.id));
    item.insert("name".to_string(), json!(call.name));
    item.insert(
        "arguments".to_string(),
        json!(call.arguments.wire_json_string()),
    );
    if let Some(provider) = provider.filter(|provider| provider.is_provider("openai")) {
        if replay.send_item_ids {
            if let Some(id) = &provider.id {
                item.insert("id".to_string(), json!(id));
            }
        }
        if let Some(namespace) = provider.details.get("namespace") {
            item.insert("namespace".to_string(), namespace.clone());
        }
        if let Some(status) = provider.details.get("status") {
            item.insert("status".to_string(), status.clone());
        }
    }
    input.push(Value::Object(item));
}

fn push_native_replay_payload(payload: &Value, input: &mut Vec<Value>) {
    let Some(item_type) = payload.get("type").and_then(Value::as_str) else {
        return;
    };
    if payload.get("id").is_none() && payload.get("call_id").is_none() {
        return;
    }
    if matches!(
        item_type,
        "web_search_call"
            | "file_search_call"
            | "image_generation_call"
            | "code_interpreter_call"
            | "mcp_call"
            | "mcp_list_tools"
            | "mcp_approval_request"
            | "tool_search_call"
            | "compaction"
    ) && !input.iter().any(|item| same_openai_item(item, payload))
    {
        input.push(payload.clone());
    }
}

fn same_openai_item(left: &Value, right: &Value) -> bool {
    let left_type = left.get("type").and_then(Value::as_str);
    let right_type = right.get("type").and_then(Value::as_str);
    if left_type != right_type {
        return false;
    }
    let left_id = left.get("id").or_else(|| left.get("call_id"));
    let right_id = right.get("id").or_else(|| right.get("call_id"));
    left_id.is_some() && left_id == right_id
}

fn strip_openai_replay_aliases(request: &mut Map<String, Value>) {
    for key in [
        "openai_previous_response_id",
        "openai_conversation_id",
        "openai_send_reasoning_ids",
        "openai_include_encrypted_reasoning",
    ] {
        request.remove(key);
    }
}

fn ensure_include(request: &mut Map<String, Value>, include: &str) {
    let entry = request
        .entry("include".to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    if let Some(items) = entry.as_array_mut() {
        if !items.iter().any(|item| item.as_str() == Some(include)) {
            items.push(Value::String(include.to_string()));
        }
    }
}

fn openai_response_details(value: &Value) -> Metadata {
    let mut details = Metadata::default();
    if let Some(status) = value.get("status").cloned() {
        details.insert("status".to_string(), status.clone());
        details.insert("finish_reason".to_string(), status);
    }
    if let Some(incomplete_details) = value.get("incomplete_details").cloned() {
        details.insert("incomplete_details".to_string(), incomplete_details);
    }
    if let Some(service_tier) = value.get("service_tier").cloned() {
        details.insert("service_tier".to_string(), service_tier);
    }
    if let Some(usage) = value.get("usage").cloned() {
        details.insert("usage".to_string(), usage);
    }
    if let Some(conversation_id) = value.get("conversation").and_then(|conversation| {
        conversation
            .as_str()
            .or_else(|| conversation.get("id").and_then(Value::as_str))
    }) {
        details.insert("conversation_id".to_string(), json!(conversation_id));
    }
    details
}

#[derive(Clone, Debug, Default)]
struct StreamedFunctionCall {
    index: usize,
    item_id: String,
    call_id: String,
    name: String,
    arguments: String,
    namespace: Option<String>,
    status: Option<String>,
    started: bool,
    ended: bool,
}

/// Incremental parser for `OpenAI` Responses server-sent JSON payloads.
#[derive(Default)]
pub struct OpenAiResponsesStreamParser {
    text_started: bool,
    text: String,
    reasoning_started: bool,
    reasoning: String,
    reasoning_item_id: Option<String>,
    reasoning_signature: Option<String>,
    reasoning_details: Metadata,
    function_calls: BTreeMap<String, StreamedFunctionCall>,
    next_tool_index: usize,
    final_seen: bool,
}

impl OpenAiResponsesStreamParser {
    /// Push one provider event and return zero or more canonical stream events.
    ///
    /// # Errors
    ///
    /// Returns an error when a completed response payload is malformed.
    pub fn push_event(
        &mut self,
        event: &Value,
    ) -> Result<Vec<ModelResponseStreamEvent>, ModelError> {
        let mut stream = Vec::new();
        match event.get("type").and_then(Value::as_str) {
            Some("response.output_text.delta") => {
                if !self.text_started {
                    self.text_started = true;
                    stream.push(ModelResponseStreamEvent::PartStart(crate::PartStart {
                        index: 0,
                        part_kind: "text".to_string(),
                    }));
                }
                if let Some(delta) = event.get("delta").and_then(Value::as_str) {
                    self.text.push_str(delta);
                    stream.push(ModelResponseStreamEvent::PartDelta(crate::PartDelta::text(
                        0, delta,
                    )));
                }
            }
            Some("response.output_text.done") if self.text_started => {
                self.end_text_part(&mut stream);
            }
            Some(
                "response.reasoning_summary_text.delta"
                | "response.reasoning_summary.delta"
                | "response.reasoning.delta",
            ) => {
                self.push_reasoning_delta(event, &mut stream);
            }
            Some(
                "response.reasoning_summary_text.done"
                | "response.reasoning_summary.done"
                | "response.reasoning.done",
            ) if self.reasoning_started => {
                self.end_reasoning_part(&mut stream);
            }
            Some("response.output_item.added") => {
                self.push_output_item_added(event, &mut stream);
            }
            Some("response.function_call_arguments.delta") => {
                self.push_function_call_arguments_delta(event, &mut stream);
            }
            Some("response.function_call_arguments.done") => {
                self.push_function_call_arguments_done(event, &mut stream);
            }
            Some("response.output_item.done") => {
                self.push_output_item_done(event, &mut stream);
            }
            Some("response.completed") => {
                self.end_open_parts(&mut stream);
                let response = event
                    .get("response")
                    .map(OpenAiResponsesAdapter::parse_response)
                    .transpose()?
                    .map_or_else(
                        || self.response_from_streamed_parts(),
                        |response| self.response_with_streamed_parts_fallback(response),
                    );
                stream.push(ModelResponseStreamEvent::FinalResult(Box::new(response)));
                self.final_seen = true;
            }
            _ => {}
        }
        Ok(stream)
    }

    fn push_reasoning_delta(&mut self, event: &Value, stream: &mut Vec<ModelResponseStreamEvent>) {
        if let Some(item_id) = event.get("item_id").and_then(Value::as_str) {
            self.reasoning_item_id = Some(item_id.to_string());
        }
        if !self.reasoning_started {
            self.reasoning_started = true;
            stream.push(ModelResponseStreamEvent::PartStart(crate::PartStart {
                index: 1,
                part_kind: "thinking".to_string(),
            }));
        }
        if let Some(delta) = event
            .get("delta")
            .or_else(|| event.get("text"))
            .and_then(Value::as_str)
        {
            self.reasoning.push_str(delta);
            stream.push(ModelResponseStreamEvent::PartDelta(
                crate::PartDelta::thinking(1, delta),
            ));
        }
    }

    fn end_text_part(&mut self, stream: &mut Vec<ModelResponseStreamEvent>) {
        stream.push(ModelResponseStreamEvent::PartEnd(crate::PartEnd {
            index: 0,
            part_kind: Some("text".to_string()),
        }));
        self.text_started = false;
    }

    fn end_reasoning_part(&mut self, stream: &mut Vec<ModelResponseStreamEvent>) {
        stream.push(ModelResponseStreamEvent::PartEnd(crate::PartEnd {
            index: 1,
            part_kind: Some("thinking".to_string()),
        }));
        self.reasoning_started = false;
    }

    fn end_open_parts(&mut self, stream: &mut Vec<ModelResponseStreamEvent>) {
        if self.reasoning_started {
            self.end_reasoning_part(stream);
        }
        if self.text_started {
            self.end_text_part(stream);
        }
    }

    fn push_output_item_added(
        &mut self,
        event: &Value,
        stream: &mut Vec<ModelResponseStreamEvent>,
    ) {
        let Some(item) = event.get("item") else {
            return;
        };
        match item.get("type").and_then(Value::as_str) {
            Some("function_call") => {
                let key = function_call_item_key(event, item);
                self.ensure_function_call_started(&key, item, stream);
                self.update_function_call_from_item(&key, item, stream, false);
            }
            Some("reasoning") => self.update_reasoning_from_item(item),
            _ => {}
        }
    }

    fn push_function_call_arguments_delta(
        &mut self,
        event: &Value,
        stream: &mut Vec<ModelResponseStreamEvent>,
    ) {
        let Some(key) = event.get("item_id").and_then(Value::as_str) else {
            return;
        };
        let key = key.to_string();
        self.ensure_function_call_started(&key, &Value::Null, stream);
        if let Some(delta) = event.get("delta").and_then(Value::as_str) {
            if delta.is_empty() {
                return;
            }
            if let Some(call) = self.function_calls.get_mut(&key) {
                call.arguments.push_str(delta);
                stream.push(ModelResponseStreamEvent::PartDelta(crate::PartDelta {
                    index: call.index,
                    delta: crate::StreamDelta::ToolCallArguments {
                        arguments_delta: delta.to_string(),
                    },
                }));
            }
        }
    }

    fn push_function_call_arguments_done(
        &mut self,
        event: &Value,
        stream: &mut Vec<ModelResponseStreamEvent>,
    ) {
        let Some(key) = event.get("item_id").and_then(Value::as_str) else {
            return;
        };
        let key = key.to_string();
        self.ensure_function_call_started(&key, &Value::Null, stream);
        let Some(arguments) = event
            .get("arguments")
            .or_else(|| event.get("delta"))
            .and_then(Value::as_str)
        else {
            return;
        };
        self.update_function_call_arguments(&key, arguments, stream);
    }

    fn push_output_item_done(&mut self, event: &Value, stream: &mut Vec<ModelResponseStreamEvent>) {
        let Some(item) = event.get("item") else {
            return;
        };
        match item.get("type").and_then(Value::as_str) {
            Some("function_call") => {
                let key = function_call_item_key(event, item);
                self.ensure_function_call_started(&key, item, stream);
                self.update_function_call_from_item(&key, item, stream, true);
                if let Some(call) = self.function_calls.get_mut(&key) {
                    if !call.ended {
                        stream.push(ModelResponseStreamEvent::PartEnd(crate::PartEnd {
                            index: call.index,
                            part_kind: Some("tool_call".to_string()),
                        }));
                        call.ended = true;
                    }
                }
            }
            Some("reasoning") => self.update_reasoning_from_item(item),
            _ => {}
        }
    }

    fn update_reasoning_from_item(&mut self, item: &Value) {
        if let Some(id) = item.get("id").and_then(Value::as_str) {
            self.reasoning_item_id = Some(id.to_string());
        }
        if let Some(encrypted_content) = item.get("encrypted_content").and_then(Value::as_str) {
            self.reasoning_signature = Some(encrypted_content.to_string());
            self.reasoning_details
                .insert("encrypted_content".to_string(), json!(encrypted_content));
        }
        if let Some(raw_content) = raw_reasoning_content(item) {
            self.reasoning_details
                .insert("raw_content".to_string(), json!(raw_content));
        }
        if self.reasoning.is_empty() {
            let summary = reasoning_summary_text(item);
            if !summary.is_empty() {
                self.reasoning = summary;
            }
        }
    }

    fn ensure_function_call_started(
        &mut self,
        key: &str,
        item: &Value,
        stream: &mut Vec<ModelResponseStreamEvent>,
    ) {
        if self.next_tool_index == 0 {
            self.next_tool_index = 2;
        }
        let mut start_index = None;
        let call = self
            .function_calls
            .entry(key.to_string())
            .or_insert_with(|| {
                let index = self.next_tool_index;
                self.next_tool_index = self.next_tool_index.saturating_add(1);
                StreamedFunctionCall {
                    index,
                    item_id: item
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or(key)
                        .to_string(),
                    call_id: item
                        .get("call_id")
                        .or_else(|| item.get("id"))
                        .and_then(Value::as_str)
                        .unwrap_or(key)
                        .to_string(),
                    name: String::new(),
                    arguments: String::new(),
                    namespace: item
                        .get("namespace")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    status: item
                        .get("status")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    started: false,
                    ended: false,
                }
            });
        if !call.started {
            call.started = true;
            start_index = Some(call.index);
        }
        if let Some(index) = start_index {
            stream.push(ModelResponseStreamEvent::PartStart(crate::PartStart {
                index,
                part_kind: "tool_call".to_string(),
            }));
        }
    }

    fn update_function_call_from_item(
        &mut self,
        key: &str,
        item: &Value,
        stream: &mut Vec<ModelResponseStreamEvent>,
        final_item: bool,
    ) {
        let Some(call) = self.function_calls.get_mut(key) else {
            return;
        };
        if let Some(item_id) = item.get("id").and_then(Value::as_str) {
            call.item_id = item_id.to_string();
        }
        if let Some(call_id) = item
            .get("call_id")
            .or_else(|| item.get("id"))
            .and_then(Value::as_str)
        {
            call.call_id = call_id.to_string();
        }
        if let Some(namespace) = item.get("namespace").and_then(Value::as_str) {
            call.namespace = Some(namespace.to_string());
        }
        if let Some(status) = item.get("status").and_then(Value::as_str) {
            call.status = Some(status.to_string());
        }
        if let Some(name) = item.get("name").and_then(Value::as_str) {
            if !name.is_empty() && call.name != name {
                call.name = name.to_string();
                stream.push(ModelResponseStreamEvent::PartDelta(crate::PartDelta {
                    index: call.index,
                    delta: crate::StreamDelta::ToolCallName {
                        name: name.to_string(),
                    },
                }));
            }
        }
        let arguments = item.get("arguments").and_then(Value::as_str);
        if let Some(arguments) = arguments {
            self.update_function_call_arguments(key, arguments, stream);
        } else if final_item && call.arguments.is_empty() {
            call.arguments = "{}".to_string();
        }
    }

    fn update_function_call_arguments(
        &mut self,
        key: &str,
        arguments: &str,
        stream: &mut Vec<ModelResponseStreamEvent>,
    ) {
        let Some(call) = self.function_calls.get_mut(key) else {
            return;
        };
        if arguments.is_empty() || call.arguments == arguments {
            return;
        }
        let delta = if call.arguments.is_empty() {
            Some(arguments.to_string())
        } else {
            arguments
                .strip_prefix(&call.arguments)
                .filter(|suffix| !suffix.is_empty())
                .map(ToString::to_string)
        };
        call.arguments = arguments.to_string();
        if let Some(arguments_delta) = delta {
            stream.push(ModelResponseStreamEvent::PartDelta(crate::PartDelta {
                index: call.index,
                delta: crate::StreamDelta::ToolCallArguments { arguments_delta },
            }));
        }
    }

    fn response_with_streamed_parts_fallback(&self, mut response: ModelResponse) -> ModelResponse {
        let has_text = !response.text_output().is_empty();
        let has_thinking = response.parts.iter().any(|part| {
            matches!(
                part,
                ModelResponsePart::Thinking { .. } | ModelResponsePart::ProviderThinking { .. }
            )
        });
        let existing_tool_keys = response
            .tool_calls()
            .into_iter()
            .map(|call| tool_call_key(&call.id, &call.name))
            .collect::<std::collections::BTreeSet<_>>();
        let mut prefix = Vec::new();
        if !has_thinking && (!self.reasoning.is_empty() || self.reasoning_signature.is_some()) {
            prefix.push(self.streamed_reasoning_part());
        }
        if !prefix.is_empty() {
            prefix.extend(response.parts);
            response.parts = prefix;
        }
        if !has_text && !self.text.is_empty() {
            response.parts.push(ModelResponsePart::Text {
                text: self.text.clone(),
            });
        }
        for part in self.streamed_tool_call_parts() {
            let Some(call) = part.tool_call() else {
                continue;
            };
            if !existing_tool_keys.contains(&tool_call_key(&call.id, &call.name)) {
                response.parts.push(part);
            }
        }
        response
    }

    fn streamed_reasoning_part(&self) -> ModelResponsePart {
        let mut provider = ProviderPartInfo::new("openai");
        if let Some(id) = &self.reasoning_item_id {
            provider = provider.with_id(id.clone());
        }
        if !self.reasoning_details.is_empty() {
            provider = provider.with_details(self.reasoning_details.clone());
        }
        ModelResponsePart::ProviderThinking {
            text: self.reasoning.clone(),
            signature: self.reasoning_signature.clone(),
            provider,
        }
    }

    fn response_from_streamed_parts(&self) -> ModelResponse {
        self.response_with_streamed_parts_fallback(ModelResponse {
            parts: Vec::new(),
            usage: Usage::default(),
            model_name: None,
            provider: Some(ProviderInfo {
                name: "openai".to_string(),
                response_id: None,
                details: serde_json::Map::new(),
            }),
            finish_reason: None,
            timestamp: None,
            run_id: None,
            conversation_id: None,
            metadata: serde_json::Map::new(),
        })
    }

    fn streamed_tool_call_parts(&self) -> Vec<ModelResponsePart> {
        let mut calls = self.function_calls.values().collect::<Vec<_>>();
        calls.sort_by_key(|call| call.index);
        calls
            .into_iter()
            .filter(|call| !call.name.is_empty())
            .map(|call| {
                let runtime_call = ToolCallPart {
                    id: if call.call_id.is_empty() {
                        call.item_id.clone()
                    } else {
                        call.call_id.clone()
                    },
                    name: call.name.clone(),
                    arguments: parse_tool_call_arguments(&Value::String(call.arguments.clone())),
                };
                let mut details = Metadata::default();
                if let Some(namespace) = &call.namespace {
                    details.insert("namespace".to_string(), json!(namespace));
                }
                if let Some(status) = &call.status {
                    details.insert("status".to_string(), json!(status));
                }
                let provider = ProviderPartInfo::new("openai")
                    .with_id(call.item_id.clone())
                    .with_details(details);
                ModelResponsePart::ProviderToolCall {
                    call: runtime_call,
                    provider,
                }
            })
            .collect()
    }

    /// Finish parsing buffered text.
    ///
    /// # Errors
    ///
    /// Returns an error when the provider stream ended without `response.completed`.
    pub fn finish(&mut self) -> Result<Vec<ModelResponseStreamEvent>, ModelError> {
        if self.final_seen {
            Ok(Vec::new())
        } else {
            Err(ModelError::ResponseParsing(
                "missing response.completed event".to_string(),
            ))
        }
    }
}

fn function_call_item_key(event: &Value, item: &Value) -> String {
    event
        .get("item_id")
        .or_else(|| item.get("id"))
        .or_else(|| item.get("call_id"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .or_else(|| {
            event
                .get("output_index")
                .and_then(Value::as_u64)
                .map(|index| format!("output-{index}"))
        })
        .unwrap_or_else(|| "function-call".to_string())
}

fn tool_call_key(id: &str, name: &str) -> String {
    if id.is_empty() {
        format!("name:{name}")
    } else {
        format!("id:{id}")
    }
}

fn parse_response_item(item: &Value, parts: &mut Vec<ModelResponsePart>) {
    match item.get("type").and_then(Value::as_str) {
        Some("message") => push_message_content_parts(item, parts),
        Some("refusal") => push_refusal_part(item, parts),
        Some("function_call") => push_function_call_part(item, parts),
        Some("reasoning") => push_reasoning_part(item, parts),
        Some(
            "web_search_call"
            | "code_interpreter_call"
            | "mcp_call"
            | "mcp_list_tools"
            | "mcp_approval_request"
            | "tool_search_call"
            | "compaction",
        ) => {
            push_native_tool_call(item, parts);
        }
        Some("image_generation_call" | "file_search_call") => {
            push_native_tool_call(item, parts);
            push_result_file_part(item, parts);
        }
        _ => {}
    }
}

fn push_message_content_parts(item: &Value, parts: &mut Vec<ModelResponsePart>) {
    let provider = provider_part_from_item(item, "openai");
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
                let provider = provider
                    .clone()
                    .with_details(output_text_details(content, item));
                parts.push(ModelResponsePart::ProviderText {
                    text: text.to_string(),
                    provider,
                });
            }
        } else if matches!(content.get("type").and_then(Value::as_str), Some("refusal")) {
            if let Some(text) = content.get("refusal").and_then(Value::as_str) {
                parts.push(ModelResponsePart::ProviderText {
                    text: text.to_string(),
                    provider: provider.clone(),
                });
            }
        }
    }
}

fn push_refusal_part(item: &Value, parts: &mut Vec<ModelResponsePart>) {
    if let Some(text) = item
        .get("refusal")
        .or_else(|| item.get("content"))
        .and_then(Value::as_str)
    {
        parts.push(ModelResponsePart::ProviderText {
            text: text.to_string(),
            provider: provider_part_from_item(item, "openai"),
        });
    }
}

fn push_function_call_part(item: &Value, parts: &mut Vec<ModelResponsePart>) {
    let mut details = Metadata::default();
    for key in ["namespace", "status"] {
        if let Some(value) = item.get(key).cloned() {
            details.insert(key.to_string(), value);
        }
    }
    let provider = provider_part_from_item(item, "openai").with_details(details);
    parts.push(ModelResponsePart::ProviderToolCall {
        call: ToolCallPart {
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
            arguments: parse_tool_call_arguments(item.get("arguments").unwrap_or(&Value::Null)),
        },
        provider,
    });
}

fn push_reasoning_part(item: &Value, parts: &mut Vec<ModelResponsePart>) {
    let text = reasoning_summary_text(item);
    let signature = item
        .get("encrypted_content")
        .and_then(Value::as_str)
        .map(str::to_string);
    let mut details = Metadata::default();
    if let Some(encrypted_content) = item.get("encrypted_content").cloned() {
        details.insert("encrypted_content".to_string(), encrypted_content);
    }
    if let Some(raw_content) = raw_reasoning_content(item) {
        details.insert("raw_content".to_string(), json!(raw_content));
    }
    if !text.is_empty() || signature.is_some() || !details.is_empty() {
        parts.push(ModelResponsePart::ProviderThinking {
            text,
            signature,
            provider: provider_part_from_item(item, "openai").with_details(details),
        });
    }
}

fn push_native_tool_call(item: &Value, parts: &mut Vec<ModelResponsePart>) {
    let item_type = item
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    parts.push(ModelResponsePart::ProviderOpaque {
        item_type,
        payload: item.clone(),
        provider: provider_part_from_item(item, "openai"),
    });
}

fn provider_part_from_item(item: &Value, provider_name: &str) -> ProviderPartInfo {
    let mut provider = ProviderPartInfo::new(provider_name.to_string());
    if let Some(id) = item.get("id").and_then(Value::as_str) {
        provider = provider.with_id(id.to_string());
    }
    provider
}

fn output_text_details(content: &Value, item: &Value) -> Metadata {
    let mut details = Metadata::default();
    for key in ["annotations", "logprobs"] {
        if let Some(value) = content.get(key).cloned() {
            details.insert(key.to_string(), value);
        }
    }
    if let Some(phase) = item.get("phase").cloned() {
        details.insert("phase".to_string(), phase);
    }
    details
}

fn reasoning_summary_text(item: &Value) -> String {
    item.get("summary")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|summary| summary.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
}

fn raw_reasoning_content(item: &Value) -> Option<Vec<String>> {
    let content = item
        .get("content")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(|content| content.get("text").and_then(Value::as_str))
        .map(str::to_string)
        .collect::<Vec<_>>();
    (!content.is_empty()).then_some(content)
}

fn push_result_file_part(item: &Value, parts: &mut Vec<ModelResponsePart>) {
    if let Some(url) = item.get("result").and_then(Value::as_str) {
        parts.push(ModelResponsePart::File {
            url: url.to_string(),
            media_type: item
                .get("media_type")
                .and_then(Value::as_str)
                .unwrap_or("application/octet-stream")
                .to_string(),
        });
    }
}

fn response_tool_defs(
    tools: &[ToolDefinition],
    native_tools: &[NativeToolDefinition],
) -> Vec<Value> {
    let mut definitions = tools
        .iter()
        .map(|tool| {
            let mut definition = serde_json::Map::new();
            definition.insert("type".to_string(), json!("function"));
            definition.insert("name".to_string(), json!(tool.name));
            insert_optional_description(&mut definition, tool.description.as_ref());
            definition.insert(
                "parameters".to_string(),
                provider_tool_parameters(&tool.parameters),
            );
            Value::Object(definition)
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::{
        ModelRequest, ModelResponsePart, ModelResponseStreamEvent, ProviderReplaySettings,
        StreamDelta, ThinkingSettings,
    };

    fn final_response(events: &[ModelResponseStreamEvent]) -> &ModelResponse {
        events
            .iter()
            .find_map(|event| match event {
                ModelResponseStreamEvent::FinalResult(response) => Some(response.as_ref()),
                _ => None,
            })
            .unwrap()
    }

    #[test]
    fn responses_stream_function_call_deltas_become_final_tool_call() {
        let events = vec![
            json!({
                "type": "response.output_item.added",
                "output_index": 0,
                "item": {
                    "id": "fc_1",
                    "type": "function_call",
                    "call_id": "call_1",
                    "name": "shell_exec",
                    "arguments": ""
                }
            }),
            json!({
                "type": "response.function_call_arguments.delta",
                "item_id": "fc_1",
                "delta": "{\"command\":\"ls"
            }),
            json!({
                "type": "response.function_call_arguments.delta",
                "item_id": "fc_1",
                "delta": "\"}"
            }),
            json!({
                "type": "response.output_item.done",
                "output_index": 0,
                "item": {
                    "id": "fc_1",
                    "type": "function_call",
                    "call_id": "call_1",
                    "name": "shell_exec",
                    "arguments": "{\"command\":\"ls\"}"
                }
            }),
            json!({
                "type": "response.completed",
                "response": {
                    "id": "resp_1",
                    "status": "completed",
                    "output": []
                }
            }),
        ];

        let stream = OpenAiResponsesAdapter::parse_stream_events(&events).unwrap();
        assert!(stream.iter().any(|event| matches!(
            event,
            ModelResponseStreamEvent::PartStart(part)
                if part.part_kind == "tool_call" && part.index == 2
        )));
        assert!(stream.iter().any(|event| matches!(
            event,
            ModelResponseStreamEvent::PartDelta(delta)
                if matches!(&delta.delta, StreamDelta::ToolCallName { name } if name == "shell_exec")
        )));
        assert!(stream.iter().any(|event| matches!(
            event,
            ModelResponseStreamEvent::PartDelta(delta)
                if matches!(&delta.delta, StreamDelta::ToolCallArguments { arguments_delta } if arguments_delta.contains("command"))
        )));

        let response = final_response(&stream);
        let tool_calls = response.tool_calls();
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].id, "call_1");
        assert_eq!(tool_calls[0].name, "shell_exec");
        assert_eq!(tool_calls[0].arguments.execution_value()["command"], "ls");
    }

    #[test]
    fn responses_stream_preserves_thinking_and_text_when_completed_output_is_empty() {
        let events = vec![
            json!({
                "type": "response.output_item.added",
                "item": {
                    "id": "rs_stream",
                    "type": "reasoning",
                    "encrypted_content": "encrypted-stream",
                    "content": [{"type": "reasoning_text", "text": "raw-stream"}]
                }
            }),
            json!({"type": "response.reasoning_summary_text.delta", "item_id": "rs_stream", "delta": "inspect"}),
            json!({"type": "response.output_text.delta", "delta": "done"}),
            json!({
                "type": "response.completed",
                "response": {
                    "id": "resp_text",
                    "status": "completed",
                    "output": []
                }
            }),
        ];

        let stream = OpenAiResponsesAdapter::parse_stream_events(&events).unwrap();
        assert!(stream.iter().any(|event| matches!(
            event,
            ModelResponseStreamEvent::PartDelta(delta)
                if matches!(&delta.delta, StreamDelta::Thinking { text } if text == "inspect")
        )));
        assert!(stream.iter().any(|event| matches!(
            event,
            ModelResponseStreamEvent::PartDelta(delta)
                if matches!(&delta.delta, StreamDelta::Text { text } if text == "done")
        )));
        let response = final_response(&stream);
        assert_eq!(response.text_output(), "done");
        assert!(response.parts.iter().any(|part| matches!(
            part,
            ModelResponsePart::ProviderThinking { text, signature, provider }
                if text == "inspect"
                    && signature.as_deref() == Some("encrypted-stream")
                    && provider.id.as_deref() == Some("rs_stream")
                    && provider.provider_name.as_deref() == Some("openai")
                    && provider.details.get("raw_content").and_then(Value::as_array).is_some_and(|items| items == &vec![json!("raw-stream")])
        )));
    }

    #[test]
    fn responses_parse_preserves_provider_replay_metadata() {
        let response = OpenAiResponsesAdapter::parse_response(&json!({
            "id": "resp_1",
            "model": "gpt-5.5",
            "status": "completed",
            "conversation": {"id": "conv_1"},
            "service_tier": "default",
            "usage": {
                "input_tokens": 10,
                "input_tokens_details": {"cached_tokens": 6},
                "output_tokens": 4,
                "output_tokens_details": {"reasoning_tokens": 2},
                "total_tokens": 14
            },
            "output": [
                {
                    "id": "msg_1",
                    "type": "message",
                    "role": "assistant",
                    "status": "completed",
                    "phase": "final_answer",
                    "content": [
                        {"type": "output_text", "text": "hello", "annotations": [{"kind": "note"}]}
                    ]
                },
                {
                    "id": "rs_1",
                    "type": "reasoning",
                    "encrypted_content": "encrypted",
                    "summary": [{"type": "summary_text", "text": "inspect"}],
                    "content": [{"type": "reasoning_text", "text": "raw"}]
                },
                {
                    "id": "fc_1",
                    "type": "function_call",
                    "call_id": "call_1",
                    "name": "lookup",
                    "arguments": "{\"q\":\"x\"}",
                    "namespace": "tools",
                    "status": "completed"
                },
                {"id": "mcp_1", "type": "mcp_call", "name": "ask", "status": "completed"}
            ]
        }))
        .unwrap();

        assert_eq!(response.usage.cache_read_tokens, 6);
        assert_eq!(
            response
                .provider
                .as_ref()
                .and_then(|provider| provider.details.get("conversation_id"))
                .and_then(Value::as_str),
            Some("conv_1")
        );
        assert!(matches!(
            &response.parts[0],
            ModelResponsePart::ProviderText { text, provider }
                if text == "hello"
                    && provider.id.as_deref() == Some("msg_1")
                    && provider.details.get("phase").and_then(Value::as_str) == Some("final_answer")
        ));
        assert!(matches!(
            &response.parts[1],
            ModelResponsePart::ProviderThinking { text, signature, provider }
                if text == "inspect"
                    && signature.as_deref() == Some("encrypted")
                    && provider.id.as_deref() == Some("rs_1")
                    && provider.details.get("raw_content").and_then(Value::as_array).is_some_and(|items| items == &vec![json!("raw")])
        ));
        assert!(matches!(
            &response.parts[2],
            ModelResponsePart::ProviderToolCall { call, provider }
                if call.id == "call_1"
                    && call.name == "lookup"
                    && call.arguments.execution_value() == json!({"q": "x"})
                    && provider.id.as_deref() == Some("fc_1")
                    && provider.details.get("namespace").and_then(Value::as_str) == Some("tools")
        ));
        assert!(matches!(
            &response.parts[3],
            ModelResponsePart::ProviderOpaque { item_type, provider, .. }
                if item_type == "mcp_call" && provider.id.as_deref() == Some("mcp_1")
        ));
    }

    #[test]
    fn responses_replay_merges_text_and_reasoning_items_by_provider_id() {
        let mut raw_details = Metadata::default();
        raw_details.insert("raw_content".to_string(), json!(["raw-a", "raw-b"]));
        let messages = vec![ModelMessage::Response(ModelResponse {
            parts: vec![
                ModelResponsePart::ProviderText {
                    text: "hello ".to_string(),
                    provider: ProviderPartInfo::new("openai").with_id("msg_1"),
                },
                ModelResponsePart::ProviderText {
                    text: "world".to_string(),
                    provider: ProviderPartInfo::new("openai").with_id("msg_1"),
                },
                ModelResponsePart::ProviderThinking {
                    text: "inspect".to_string(),
                    signature: Some("encrypted".to_string()),
                    provider: ProviderPartInfo::new("openai")
                        .with_id("rs_1")
                        .with_details(raw_details),
                },
                ModelResponsePart::ProviderThinking {
                    text: "decide".to_string(),
                    signature: None,
                    provider: ProviderPartInfo::new("openai").with_id("rs_1"),
                },
            ],
            usage: Usage::default(),
            model_name: None,
            provider: Some(ProviderInfo {
                name: "openai".to_string(),
                response_id: Some("resp_1".to_string()),
                details: Metadata::default(),
            }),
            finish_reason: None,
            timestamp: None,
            run_id: None,
            conversation_id: None,
            metadata: Metadata::default(),
        })];
        let settings = ModelSettings {
            provider_replay: Some(ProviderReplaySettings {
                include_encrypted_reasoning: Some(true),
                ..ProviderReplaySettings::default()
            }),
            ..ModelSettings::default()
        };

        let request =
            OpenAiResponsesAdapter::build_request("gpt-5.5", &messages, Some(&settings), &[], &[])
                .unwrap();

        assert_eq!(request["input"].as_array().unwrap().len(), 2);
        assert_eq!(request["input"][0]["id"], "msg_1");
        assert_eq!(request["input"][0]["content"].as_array().unwrap().len(), 2);
        assert_eq!(request["input"][0]["content"][0]["text"], "hello ");
        assert_eq!(request["input"][0]["content"][1]["text"], "world");
        assert_eq!(request["input"][1]["id"], "rs_1");
        assert_eq!(request["input"][1]["encrypted_content"], "encrypted");
        assert_eq!(request["input"][1]["summary"].as_array().unwrap().len(), 2);
        assert_eq!(request["input"][1]["content"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn responses_previous_response_auto_keeps_static_instructions_after_trimming() {
        let mut dynamic_metadata = Metadata::default();
        dynamic_metadata.insert(
            "starweaver_instruction_origin".to_string(),
            json!("runtime_context"),
        );
        let messages = vec![
            ModelMessage::Request(ModelRequest {
                parts: vec![
                    ModelRequestPart::SystemPrompt {
                        text: "stable system".to_string(),
                        metadata: Metadata::default(),
                    },
                    ModelRequestPart::UserPrompt {
                        content: vec![crate::message::ContentPart::Text {
                            text: "old".to_string(),
                        }],
                        name: None,
                        metadata: Metadata::default(),
                    },
                ],
                timestamp: None,
                instructions: None,
                run_id: None,
                conversation_id: None,
                metadata: Metadata::default(),
            }),
            openai_response_with_id("resp_1"),
            ModelMessage::Request(ModelRequest {
                parts: vec![
                    ModelRequestPart::Instruction {
                        text: "<runtime-context><current-time>now</current-time></runtime-context>"
                            .to_string(),
                        metadata: dynamic_metadata,
                    },
                    ModelRequestPart::UserPrompt {
                        content: vec![crate::message::ContentPart::Text {
                            text: "new".to_string(),
                        }],
                        name: None,
                        metadata: Metadata::default(),
                    },
                ],
                timestamp: None,
                instructions: None,
                run_id: None,
                conversation_id: None,
                metadata: Metadata::default(),
            }),
        ];
        let settings = ModelSettings {
            provider_replay: Some(ProviderReplaySettings {
                previous_response_id: Some("auto".to_string()),
                ..ProviderReplaySettings::default()
            }),
            ..ModelSettings::default()
        };

        let request =
            OpenAiResponsesAdapter::build_request("gpt-5.5", &messages, Some(&settings), &[], &[])
                .unwrap();

        assert_eq!(request["previous_response_id"], "resp_1");
        assert_eq!(request["instructions"], "stable system");
        assert!(!request["instructions"]
            .as_str()
            .unwrap()
            .contains("runtime-context"));
        let input = request["input"].as_array().unwrap();
        assert_eq!(input.len(), 2);
        assert_eq!(input[0]["role"], "system");
        assert!(input[0]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("runtime-context"));
        assert_eq!(input[1]["role"], "user");
        assert_eq!(input[1]["content"][0]["text"], "new");
    }

    #[test]
    fn responses_previous_response_auto_trims_after_latest_same_provider_response() {
        let messages = vec![
            ModelMessage::Request(ModelRequest::user_text("old")),
            openai_response_with_id("resp_1"),
            ModelMessage::Request(ModelRequest::user_text("new")),
        ];
        let settings = ModelSettings {
            provider_replay: Some(ProviderReplaySettings {
                previous_response_id: Some("auto".to_string()),
                ..ProviderReplaySettings::default()
            }),
            ..ModelSettings::default()
        };

        let request =
            OpenAiResponsesAdapter::build_request("gpt-5.5", &messages, Some(&settings), &[], &[])
                .unwrap();

        assert_eq!(request["previous_response_id"], "resp_1");
        assert_eq!(request["input"].as_array().unwrap().len(), 1);
        assert_eq!(request["input"][0]["content"][0]["text"], "new");
    }

    #[test]
    fn responses_previous_response_auto_does_not_cross_compaction_boundary() {
        let mut compaction = openai_response_with_id("resp_compact");
        if let ModelMessage::Response(response) = &mut compaction {
            response
                .provider
                .as_mut()
                .unwrap()
                .details
                .insert("compaction".to_string(), json!(true));
        }
        let messages = vec![
            ModelMessage::Request(ModelRequest::user_text("old")),
            compaction,
            ModelMessage::Request(ModelRequest::user_text("new")),
        ];
        let settings = ModelSettings {
            provider_replay: Some(ProviderReplaySettings {
                previous_response_id: Some("auto".to_string()),
                ..ProviderReplaySettings::default()
            }),
            ..ModelSettings::default()
        };

        let request =
            OpenAiResponsesAdapter::build_request("gpt-5.5", &messages, Some(&settings), &[], &[])
                .unwrap();

        assert!(request.get("previous_response_id").is_none());
        assert_eq!(request["input"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn responses_conversation_auto_and_concrete_trim_history() {
        let messages = vec![
            ModelMessage::Request(ModelRequest::user_text("old")),
            openai_response_with_conversation("conv_1"),
            ModelMessage::Request(ModelRequest::user_text("new")),
        ];
        let auto_settings = ModelSettings {
            provider_replay: Some(ProviderReplaySettings {
                conversation_id: Some("auto".to_string()),
                ..ProviderReplaySettings::default()
            }),
            ..ModelSettings::default()
        };
        let auto_request = OpenAiResponsesAdapter::build_request(
            "gpt-5.5",
            &messages,
            Some(&auto_settings),
            &[],
            &[],
        )
        .unwrap();
        assert_eq!(auto_request["conversation"], "conv_1");
        assert_eq!(auto_request["input"].as_array().unwrap().len(), 1);

        let concrete_settings = ModelSettings {
            provider_replay: Some(ProviderReplaySettings {
                conversation_id: Some("conv_1".to_string()),
                ..ProviderReplaySettings::default()
            }),
            ..ModelSettings::default()
        };
        let concrete_request = OpenAiResponsesAdapter::build_request(
            "gpt-5.5",
            &messages,
            Some(&concrete_settings),
            &[],
            &[],
        )
        .unwrap();
        assert_eq!(concrete_request["conversation"], "conv_1");
        assert_eq!(concrete_request["input"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn responses_server_side_state_rejects_previous_response_and_conversation_conflict() {
        let settings = ModelSettings {
            provider_replay: Some(ProviderReplaySettings {
                previous_response_id: Some("auto".to_string()),
                conversation_id: Some("auto".to_string()),
                ..ProviderReplaySettings::default()
            }),
            ..ModelSettings::default()
        };
        let error =
            OpenAiResponsesAdapter::build_request("gpt-5.5", &[], Some(&settings), &[], &[])
                .unwrap_err();

        assert!(
            matches!(error, ModelError::MessageMapping(message) if message.contains("cannot both be set"))
        );
    }

    #[test]
    fn responses_request_includes_encrypted_reasoning_when_thinking_is_enabled() {
        let settings = ModelSettings {
            thinking: Some(ThinkingSettings {
                effort: "high".to_string(),
                budget_tokens: None,
                mode: None,
                include_thoughts: None,
                summary: Some("auto".to_string()),
            }),
            ..ModelSettings::default()
        };

        let request = OpenAiResponsesAdapter::build_request(
            "gpt-5.5",
            &[ModelMessage::Request(ModelRequest::user_text("think"))],
            Some(&settings),
            &[],
            &[],
        )
        .unwrap();

        assert_eq!(request["include"], json!(["reasoning.encrypted_content"]));
        assert_eq!(request["reasoning"]["effort"], "high");
        assert_eq!(request["reasoning"]["summary"], "auto");
    }

    #[test]
    fn responses_stream_requires_completed_event() {
        let error = OpenAiResponsesAdapter::parse_stream_events(&[
            json!({"type": "response.output_text.delta", "delta": "partial"}),
        ])
        .unwrap_err();

        assert!(
            matches!(error, ModelError::ResponseParsing(message) if message.contains("missing response.completed"))
        );
    }

    #[test]
    fn responses_send_item_ids_false_does_not_default_encrypted_reasoning_include() {
        let settings = ModelSettings {
            thinking: Some(ThinkingSettings {
                effort: "high".to_string(),
                budget_tokens: None,
                mode: None,
                include_thoughts: None,
                summary: Some("auto".to_string()),
            }),
            provider_replay: Some(ProviderReplaySettings {
                send_item_ids: Some(false),
                ..ProviderReplaySettings::default()
            }),
            ..ModelSettings::default()
        };

        let request = OpenAiResponsesAdapter::build_request(
            "gpt-5.5",
            &[ModelMessage::Request(ModelRequest::user_text("think"))],
            Some(&settings),
            &[],
            &[],
        )
        .unwrap();

        assert!(request.get("include").is_none());
        assert_eq!(request["reasoning"]["effort"], "high");
    }

    #[test]
    fn responses_replay_omits_encrypted_reasoning_when_disabled() {
        let messages = vec![ModelMessage::Response(ModelResponse {
            parts: vec![ModelResponsePart::ProviderThinking {
                text: "inspect".to_string(),
                signature: Some("encrypted".to_string()),
                provider: ProviderPartInfo::new("openai")
                    .with_id("rs_1")
                    .with_details({
                        let mut details = Metadata::default();
                        details.insert("raw_content".to_string(), json!(["raw"]));
                        details
                    }),
            }],
            usage: Usage::default(),
            model_name: None,
            provider: None,
            finish_reason: None,
            timestamp: None,
            run_id: None,
            conversation_id: None,
            metadata: Metadata::default(),
        })];
        let settings = ModelSettings {
            provider_replay: Some(ProviderReplaySettings {
                include_encrypted_reasoning: Some(false),
                ..ProviderReplaySettings::default()
            }),
            ..ModelSettings::default()
        };

        let request =
            OpenAiResponsesAdapter::build_request("gpt-5.5", &messages, Some(&settings), &[], &[])
                .unwrap();

        assert_eq!(request["input"][0]["type"], "reasoning");
        assert_eq!(request["input"][0]["id"], "rs_1");
        assert!(request["input"][0].get("encrypted_content").is_none());
        assert_eq!(request["input"][0]["content"][0]["text"], "raw");
        assert!(request.get("include").is_none());
    }

    #[test]
    fn responses_replay_send_item_ids_false_uses_safe_visible_fallbacks() {
        let messages = vec![ModelMessage::Response(ModelResponse {
            parts: vec![
                ModelResponsePart::ProviderText {
                    text: "hello".to_string(),
                    provider: ProviderPartInfo::new("openai").with_id("msg_1"),
                },
                ModelResponsePart::ProviderThinking {
                    text: "inspect".to_string(),
                    signature: Some("encrypted".to_string()),
                    provider: ProviderPartInfo::new("openai").with_id("rs_1"),
                },
                ModelResponsePart::ProviderOpaque {
                    item_type: "mcp_call".to_string(),
                    payload: json!({"type": "mcp_call", "id": "mcp_1", "status": "completed"}),
                    provider: ProviderPartInfo::new("openai").with_id("mcp_1"),
                },
            ],
            usage: Usage::default(),
            model_name: None,
            provider: None,
            finish_reason: None,
            timestamp: None,
            run_id: None,
            conversation_id: None,
            metadata: Metadata::default(),
        })];
        let settings = ModelSettings {
            provider_replay: Some(ProviderReplaySettings {
                send_item_ids: Some(false),
                include_encrypted_reasoning: Some(false),
                ..ProviderReplaySettings::default()
            }),
            ..ModelSettings::default()
        };

        let request =
            OpenAiResponsesAdapter::build_request("gpt-5.5", &messages, Some(&settings), &[], &[])
                .unwrap();

        let input = request["input"].as_array().unwrap();
        assert_eq!(input.len(), 2);
        assert_eq!(input[0]["role"], "assistant");
        assert_eq!(input[0]["content"][0]["text"], "hello");
        assert_eq!(input[1]["content"][0]["text"], "<think>\ninspect\n</think>");
        let serialized = serde_json::to_string(&request).unwrap();
        assert!(!serialized.contains("msg_1"));
        assert!(!serialized.contains("rs_1"));
        assert!(!serialized.contains("mcp_1"));
        assert!(!serialized.contains("encrypted"));
        assert!(!serialized.contains("mcp_call"));
    }

    #[test]
    fn responses_replays_cross_provider_thinking_as_tagged_text() {
        let messages = vec![ModelMessage::Response(ModelResponse {
            parts: vec![ModelResponsePart::ProviderThinking {
                text: "other reasoning".to_string(),
                signature: Some("foreign".to_string()),
                provider: ProviderPartInfo::new("anthropic").with_id("think_1"),
            }],
            usage: Usage::default(),
            model_name: None,
            provider: None,
            finish_reason: None,
            timestamp: None,
            run_id: None,
            conversation_id: None,
            metadata: Metadata::default(),
        })];

        let request =
            OpenAiResponsesAdapter::build_request("gpt-5.5", &messages, None, &[], &[]).unwrap();

        assert_eq!(
            request["input"][0]["content"][0]["text"],
            "<think>\nother reasoning\n</think>"
        );
    }

    fn openai_response_with_id(id: &str) -> ModelMessage {
        ModelMessage::Response(ModelResponse {
            parts: vec![ModelResponsePart::ProviderText {
                text: "stored".to_string(),
                provider: ProviderPartInfo::new("openai").with_id("msg_stored"),
            }],
            usage: Usage::default(),
            model_name: None,
            provider: Some(ProviderInfo {
                name: "openai".to_string(),
                response_id: Some(id.to_string()),
                details: Metadata::default(),
            }),
            finish_reason: None,
            timestamp: None,
            run_id: None,
            conversation_id: None,
            metadata: Metadata::default(),
        })
    }

    fn openai_response_with_conversation(conversation_id: &str) -> ModelMessage {
        let mut details = Metadata::default();
        details.insert("conversation_id".to_string(), json!(conversation_id));
        ModelMessage::Response(ModelResponse {
            parts: vec![ModelResponsePart::ProviderText {
                text: "stored".to_string(),
                provider: ProviderPartInfo::new("openai").with_id("msg_stored"),
            }],
            usage: Usage::default(),
            model_name: None,
            provider: Some(ProviderInfo {
                name: "openai".to_string(),
                response_id: Some("resp_1".to_string()),
                details,
            }),
            finish_reason: None,
            timestamp: None,
            run_id: None,
            conversation_id: None,
            metadata: Metadata::default(),
        })
    }
}
