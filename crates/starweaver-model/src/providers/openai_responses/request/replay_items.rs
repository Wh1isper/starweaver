use serde_json::{Map, Value, json};

use crate::message::{ModelResponse, ModelResponsePart, ProviderPartInfo, ToolCallPart};

use super::options::OpenAiReplayOptions;

pub(super) fn push_response_replay_items(
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

pub(super) fn response_replay_items(
    response: &ModelResponse,
    replay: &OpenAiReplayOptions,
) -> Vec<Value> {
    let mut input = Vec::new();
    push_response_replay_items(response, replay, &mut input);
    input
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
        if replay.send_item_ids
            && let Some(id) = &provider.id
        {
            item.insert("id".to_string(), json!(id));
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
