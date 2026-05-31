//! Wire mappers for the first supported provider protocol families.

pub mod anthropic;
pub mod bedrock;
pub mod client;
pub mod gemini;
pub mod openai_chat;
pub mod openai_responses;

use serde_json::{json, Map, Value};

use crate::{
    message::{ContentPart, FinishReason, ModelMessage, ModelRequestPart},
    settings::ToolChoice,
    ModelSettings,
};

fn text_from_content(content: &[ContentPart]) -> String {
    content
        .iter()
        .filter_map(|part| match part {
            ContentPart::Text { text } => Some(text.as_str()),
            ContentPart::ImageUrl { .. } | ContentPart::FileUrl { .. } => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

pub(crate) fn openai_chat_content(content: &[ContentPart]) -> Value {
    if content.len() == 1 {
        if let ContentPart::Text { text } = &content[0] {
            return json!(text);
        }
    }
    Value::Array(
        content
            .iter()
            .map(|part| match part {
                ContentPart::Text { text } => json!({"type": "text", "text": text}),
                ContentPart::ImageUrl { url } => {
                    json!({"type": "image_url", "image_url": {"url": url}})
                }
                ContentPart::FileUrl { url, media_type } => json!({
                    "type": "file",
                    "file": {"file_data": url, "media_type": media_type}
                }),
            })
            .collect(),
    )
}

pub(crate) fn openai_responses_content(content: &[ContentPart]) -> Vec<Value> {
    content
        .iter()
        .map(|part| match part {
            ContentPart::Text { text } => json!({"type": "input_text", "text": text}),
            ContentPart::ImageUrl { url } => json!({"type": "input_image", "image_url": url}),
            ContentPart::FileUrl { url, media_type } => json!({
                "type": "input_file",
                "file_url": url,
                "media_type": media_type
            }),
        })
        .collect()
}

pub(crate) fn gemini_parts_from_content(content: &[ContentPart]) -> Vec<Value> {
    content
        .iter()
        .map(|part| match part {
            ContentPart::Text { text } => json!({"text": text}),
            ContentPart::ImageUrl { url } => json!({
                "fileData": {"fileUri": url, "mimeType": "image/*"}
            }),
            ContentPart::FileUrl { url, media_type } => json!({
                "fileData": {"fileUri": url, "mimeType": media_type}
            }),
        })
        .collect()
}

pub(crate) fn bedrock_content_from_content(content: &[ContentPart]) -> Vec<Value> {
    content
        .iter()
        .map(|part| match part {
            ContentPart::Text { text } => json!({"text": text}),
            ContentPart::ImageUrl { url } => json!({"image": {"source": {"bytes": url}}}),
            ContentPart::FileUrl { url, media_type } => json!({
                "document": {
                    "format": media_type,
                    "source": {"bytes": url},
                }
            }),
        })
        .collect()
}

fn collect_system_and_non_system(messages: &[ModelMessage]) -> (Vec<String>, Vec<&ModelMessage>) {
    let mut system = Vec::new();
    let mut rest = Vec::new();

    for message in messages {
        match message {
            ModelMessage::Request(request) => {
                let mut has_non_system = false;
                for part in &request.parts {
                    match part {
                        ModelRequestPart::SystemPrompt { text, .. }
                        | ModelRequestPart::Instruction { text, .. } => system.push(text.clone()),
                        _ => has_non_system = true,
                    }
                }
                if has_non_system {
                    rest.push(message);
                }
            }
            ModelMessage::Response(_) => rest.push(message),
        }
    }

    (system, rest)
}

fn usage_from_openai(value: &Value) -> starweaver_core::Usage {
    let usage = value.get("usage");
    starweaver_core::Usage {
        requests: 1,
        input_tokens: usage
            .and_then(|u| u.get("prompt_tokens").or_else(|| u.get("input_tokens")))
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        output_tokens: usage
            .and_then(|u| {
                u.get("completion_tokens")
                    .or_else(|| u.get("output_tokens"))
            })
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        total_tokens: usage
            .and_then(|u| u.get("total_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        tool_calls: 0,
    }
}

fn usage_from_named(value: &Value, input: &str, output: &str) -> starweaver_core::Usage {
    let usage = value.get("usage").or_else(|| value.get("usageMetadata"));
    let input_tokens = usage
        .and_then(|u| u.get(input))
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let output_tokens = usage
        .and_then(|u| u.get(output))
        .and_then(Value::as_u64)
        .unwrap_or_default();
    starweaver_core::Usage {
        requests: 1,
        input_tokens,
        output_tokens,
        total_tokens: usage
            .and_then(|u| u.get("totalTokens").or_else(|| u.get("total_tokens")))
            .and_then(Value::as_u64)
            .unwrap_or(input_tokens + output_tokens),
        tool_calls: 0,
    }
}

fn finish_reason_openai(reason: &str) -> FinishReason {
    match reason {
        "stop" | "completed" => FinishReason::Stop,
        "length" => FinishReason::Length,
        "tool_calls" => FinishReason::ToolCalls,
        "content_filter" => FinishReason::ContentFilter,
        _ => FinishReason::Unknown,
    }
}

pub(crate) fn provider_tool_parameters(parameters: &Value) -> Value {
    let mut schema = parameters.clone();
    remove_schema_meta(&mut schema);
    schema
}

pub(crate) fn insert_optional_description(
    object: &mut Map<String, Value>,
    description: Option<&String>,
) {
    if let Some(description) = description
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty())
    {
        object.insert("description".to_string(), json!(description));
    }
}

fn remove_schema_meta(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.remove("$schema");
            for nested in object.values_mut() {
                remove_schema_meta(nested);
            }
        }
        Value::Array(items) => {
            for item in items {
                remove_schema_meta(item);
            }
        }
        _ => {}
    }
}

fn apply_common_settings(
    target: &mut serde_json::Map<String, Value>,
    settings: Option<&ModelSettings>,
) {
    if let Some(settings) = settings {
        if let Some(max_tokens) = settings.max_tokens {
            target.insert("max_tokens".to_string(), json!(max_tokens));
        }
        if let Some(temperature) = settings.temperature {
            target.insert("temperature".to_string(), json!(temperature));
        }
        if let Some(top_p) = settings.top_p {
            target.insert("top_p".to_string(), json!(top_p));
        }
        if !settings.stop_sequences.is_empty() {
            target.insert("stop".to_string(), json!(settings.stop_sequences));
        }
        if let Some(parallel_tool_calls) = settings.parallel_tool_calls {
            target.insert(
                "parallel_tool_calls".to_string(),
                json!(parallel_tool_calls),
            );
        }
        if let Some(options) = settings
            .provider_options
            .as_ref()
            .and_then(Value::as_object)
        {
            for (key, value) in options {
                target.insert(key.clone(), value.clone());
            }
        }
    }
}

pub(crate) fn openai_chat_tool_choice(choice: &ToolChoice) -> Value {
    match choice {
        ToolChoice::Auto => json!("auto"),
        ToolChoice::None => json!("none"),
        ToolChoice::Required => json!("required"),
        ToolChoice::Tool { name } => json!({
            "type": "function",
            "function": {"name": name}
        }),
    }
}

pub(crate) fn openai_responses_tool_choice(choice: &ToolChoice) -> Value {
    match choice {
        ToolChoice::Auto => json!("auto"),
        ToolChoice::None => json!("none"),
        ToolChoice::Required => json!("required"),
        ToolChoice::Tool { name } => json!({
            "type": "function",
            "name": name,
        }),
    }
}

fn parse_tool_call_arguments(value: &Value) -> Value {
    value
        .as_str()
        .and_then(|raw| serde_json::from_str(raw).ok())
        .unwrap_or_else(|| value.clone())
}
