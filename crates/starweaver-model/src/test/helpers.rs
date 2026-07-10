use serde_json::Value;
use starweaver_usage::Usage;

use crate::{
    message::{
        ContentPart, ModelMessage, ModelRequestPart, ModelResponse, ModelResponsePart, ToolCallPart,
    },
    request::context_origin_metadata,
};

/// Create a tool call response for tests.
#[must_use]
pub fn tool_call_response(
    id: impl Into<String>,
    name: impl Into<String>,
    arguments: Value,
) -> ModelResponse {
    ModelResponse {
        parts: vec![ModelResponsePart::ToolCall(ToolCallPart {
            id: id.into(),
            name: name.into(),
            arguments: arguments.into(),
        })],
        usage: Usage::default(),
        model_name: Some("test".to_string()),
        provider: None,
        finish_reason: None,
        timestamp: None,
        run_id: None,
        conversation_id: None,
        metadata: serde_json::Map::new(),
    }
}

/// Return the latest user prompt text from canonical history.
#[must_use]
pub fn latest_user_text(messages: &[ModelMessage]) -> Option<String> {
    messages.iter().rev().find_map(|message| match message {
        ModelMessage::Request(request) => request.parts.iter().rev().find_map(|part| match part {
            ModelRequestPart::UserPrompt {
                content, metadata, ..
            } if context_origin_metadata(metadata).is_none() => Some(text_from_content(content)),
            ModelRequestPart::RetryPrompt { text, .. }
            | ModelRequestPart::Instruction { text, .. }
            | ModelRequestPart::SystemPrompt { text, .. } => Some(text.clone()),
            ModelRequestPart::UserPrompt { .. } | ModelRequestPart::ToolReturn(_) => None,
        }),
        ModelMessage::Response(_) => None,
    })
}

fn text_from_content(content: &[ContentPart]) -> String {
    content
        .iter()
        .filter_map(|part| match part {
            ContentPart::CachePoint { .. } => None,
            ContentPart::Text { text } => Some(text.as_str()),
            ContentPart::ImageUrl { url } | ContentPart::FileUrl { url, .. } => Some(url.as_str()),
            ContentPart::Binary { media_type, .. }
            | ContentPart::ResourceRef { media_type, .. }
            | ContentPart::DataUrl { media_type, .. } => Some(media_type.as_str()),
        })
        .collect::<Vec<_>>()
        .join("\n")
}
