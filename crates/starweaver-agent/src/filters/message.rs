//! Shared message and request metadata helpers for SDK filters.

use serde_json::{json, Map, Value};
use starweaver_model::{ContentPart, ModelMessage, ModelRequest, ModelRequestPart};

const FILTER_ORDER_METADATA: &str = "starweaver_filter_order";

pub(super) fn metadata_text(metadata: &Map<String, Value>, key: &str) -> Option<String> {
    match metadata.get(key)? {
        Value::String(text) => Some(text.clone()),
        Value::Array(items) => Some(
            items
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        other => Some(other.to_string()),
    }
    .filter(|text| !text.trim().is_empty())
}

pub(super) fn push_user_text(messages: &mut Vec<ModelMessage>, text: String, source: &str) {
    let request = ModelRequest {
        parts: vec![ModelRequestPart::UserPrompt {
            content: vec![ContentPart::Text { text }],
            name: None,
            metadata: Map::from_iter([("starweaver_filter_source".to_string(), json!(source))]),
        }],
        timestamp: None,
        instructions: None,
        run_id: None,
        conversation_id: None,
        metadata: Map::new(),
    };
    messages.push(ModelMessage::Request(request));
}

pub(super) fn insert_request_part_after_control_parts(
    messages: &mut Vec<ModelMessage>,
    part: ModelRequestPart,
) {
    for message in messages.iter_mut().rev() {
        if let ModelMessage::Request(request) = message {
            let insert_at = request
                .parts
                .iter()
                .enumerate()
                .filter_map(|(index, part)| match part {
                    ModelRequestPart::ToolReturn(_) | ModelRequestPart::RetryPrompt { .. } => {
                        Some(index + 1)
                    }
                    ModelRequestPart::SystemPrompt { .. }
                    | ModelRequestPart::UserPrompt { .. }
                    | ModelRequestPart::Instruction { .. } => None,
                })
                .next_back()
                .unwrap_or(0);
            request.parts.insert(insert_at, part);
            return;
        }
    }
    messages.push(ModelMessage::Request(ModelRequest {
        parts: vec![part],
        timestamp: None,
        instructions: None,
        run_id: None,
        conversation_id: None,
        metadata: Map::new(),
    }));
}

pub(super) fn request_metadata_mut(messages: &mut Vec<ModelMessage>) -> &mut Map<String, Value> {
    let needs_request = !matches!(messages.last(), Some(ModelMessage::Request(_)));
    if needs_request {
        messages.push(ModelMessage::Request(ModelRequest {
            parts: Vec::new(),
            timestamp: None,
            instructions: None,
            run_id: None,
            conversation_id: None,
            metadata: Map::new(),
        }));
    }
    match messages.last_mut() {
        Some(ModelMessage::Request(request)) => &mut request.metadata,
        Some(ModelMessage::Response(_)) | None => unreachable!("request metadata ensured"),
    }
}

pub(super) fn record_filter_order(messages: &mut Vec<ModelMessage>, name: &str) {
    let entry = request_metadata_mut(messages)
        .entry(FILTER_ORDER_METADATA.to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    if let Some(items) = entry.as_array_mut() {
        items.push(Value::String(name.to_string()));
    }
}
