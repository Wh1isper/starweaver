//! `OpenAI` Responses completed response parsing.

use serde_json::{Value, json};

use crate::{
    ModelError,
    message::{
        Metadata, ModelResponse, ModelResponsePart, ProviderInfo, ProviderPartInfo, ToolCallPart,
    },
    providers::{finish_reason_openai, parse_tool_call_arguments, usage_from_openai},
};

#[allow(clippy::unnecessary_wraps)]
pub(super) fn parse_response(value: &Value) -> Result<ModelResponse, ModelError> {
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
            | "custom_tool_call"
            | "custom_tool_call_output"
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
        } else if matches!(content.get("type").and_then(Value::as_str), Some("refusal"))
            && let Some(text) = content.get("refusal").and_then(Value::as_str)
        {
            parts.push(ModelResponsePart::ProviderText {
                text: text.to_string(),
                provider: provider.clone(),
            });
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

pub(super) fn reasoning_summary_text(item: &Value) -> String {
    item.get("summary")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|summary| summary.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn raw_reasoning_content(item: &Value) -> Option<Vec<String>> {
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
