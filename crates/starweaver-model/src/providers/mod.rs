//! Wire mappers for the first supported provider protocol families.

pub mod anthropic;
pub mod bedrock;
pub mod client;
pub mod gemini;
pub mod openai_chat;
pub mod openai_responses;

use serde_json::{json, Value};

use crate::{
    message::{ContentPart, FinishReason, ModelMessage, ModelRequestPart},
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
    }
}

fn parse_tool_call_arguments(value: &Value) -> Value {
    value
        .as_str()
        .and_then(|raw| serde_json::from_str(raw).ok())
        .unwrap_or_else(|| value.clone())
}
