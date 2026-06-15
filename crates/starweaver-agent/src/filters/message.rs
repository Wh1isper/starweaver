//! Shared message and request metadata helpers for SDK filters.

use serde_json::{json, Map, Value};
use starweaver_model::{
    ContentPart, ModelMessage, ModelRequest, ModelRequestPart, INSTRUCTION_DYNAMIC_METADATA,
    INSTRUCTION_ORIGIN_ENVIRONMENT_CONTEXT, INSTRUCTION_ORIGIN_HANDOFF,
    INSTRUCTION_ORIGIN_METADATA, INSTRUCTION_ORIGIN_RUNTIME_CONTEXT,
};

const FILTER_ORDER_METADATA: &str = "starweaver_filter_order";
const TOOL_RETURN_MEDIA_ORIGIN: &str = "tool_return_media";

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
            let insert_at = request_instruction_insert_index(request);
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

pub(super) fn insert_context_part_after_control_parts(
    messages: &mut Vec<ModelMessage>,
    part: ModelRequestPart,
) {
    for message in messages.iter_mut().rev() {
        if let ModelMessage::Request(request) = message {
            let insert_at = request_context_insert_index(request);
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

fn request_instruction_insert_index(request: &ModelRequest) -> usize {
    let control_prefix_len = request_control_prefix_len(request);
    control_prefix_len
        + request.parts[control_prefix_len..]
            .iter()
            .take_while(|part| is_static_instruction_prefix_part(part))
            .count()
}

fn request_context_insert_index(request: &ModelRequest) -> usize {
    let instruction_end = request_instruction_end_index(request);
    instruction_end
        + request.parts[instruction_end..]
            .iter()
            .take_while(|part| is_context_user_prompt(part))
            .count()
}

fn request_instruction_end_index(request: &ModelRequest) -> usize {
    let control_prefix_len = request_control_prefix_len(request);
    control_prefix_len
        + request.parts[control_prefix_len..]
            .iter()
            .take_while(|part| is_instruction_prefix_part(part))
            .count()
}

fn request_control_prefix_len(request: &ModelRequest) -> usize {
    request
        .parts
        .iter()
        .take_while(|part| is_control_prefix_part(part))
        .count()
}

fn is_control_prefix_part(part: &ModelRequestPart) -> bool {
    match part {
        ModelRequestPart::ToolReturn(_) | ModelRequestPart::RetryPrompt { .. } => true,
        ModelRequestPart::UserPrompt { metadata, .. } => metadata
            .get("starweaver_instruction_origin")
            .and_then(Value::as_str)
            .is_some_and(|origin| origin == TOOL_RETURN_MEDIA_ORIGIN),
        ModelRequestPart::SystemPrompt { .. } | ModelRequestPart::Instruction { .. } => false,
    }
}

fn is_static_instruction_prefix_part(part: &ModelRequestPart) -> bool {
    match part {
        ModelRequestPart::SystemPrompt { .. } => true,
        ModelRequestPart::Instruction { metadata, .. } => !metadata
            .get(INSTRUCTION_DYNAMIC_METADATA)
            .and_then(Value::as_bool)
            .unwrap_or(false),
        ModelRequestPart::UserPrompt { .. }
        | ModelRequestPart::ToolReturn(_)
        | ModelRequestPart::RetryPrompt { .. } => false,
    }
}

const fn is_instruction_prefix_part(part: &ModelRequestPart) -> bool {
    matches!(
        part,
        ModelRequestPart::SystemPrompt { .. } | ModelRequestPart::Instruction { .. }
    )
}

fn is_context_user_prompt(part: &ModelRequestPart) -> bool {
    match part {
        ModelRequestPart::UserPrompt { metadata, .. } => metadata
            .get(INSTRUCTION_ORIGIN_METADATA)
            .and_then(Value::as_str)
            .is_some_and(|origin| {
                matches!(
                    origin,
                    INSTRUCTION_ORIGIN_ENVIRONMENT_CONTEXT
                        | INSTRUCTION_ORIGIN_RUNTIME_CONTEXT
                        | INSTRUCTION_ORIGIN_HANDOFF
                )
            }),
        ModelRequestPart::SystemPrompt { .. }
        | ModelRequestPart::Instruction { .. }
        | ModelRequestPart::ToolReturn(_)
        | ModelRequestPart::RetryPrompt { .. } => false,
    }
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

pub(super) fn metadata_content_parts(
    metadata: &Map<String, Value>,
    key: &str,
) -> Option<Vec<ContentPart>> {
    serde_json::from_value(metadata.get(key)?.clone())
        .ok()
        .filter(|content: &Vec<ContentPart>| !content.is_empty())
}

pub(super) fn metadata_string_array(metadata: &Map<String, Value>, key: &str) -> Vec<String> {
    match metadata.get(key) {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect(),
        Some(Value::String(text)) => vec![text.clone()],
        Some(other) => vec![other.to_string()],
        None => Vec::new(),
    }
}

pub(super) fn text_user_part(text: impl Into<String>) -> ModelRequestPart {
    ModelRequestPart::UserPrompt {
        content: vec![ContentPart::Text { text: text.into() }],
        name: None,
        metadata: Map::new(),
    }
}

pub(super) fn build_context_restored_part() -> ModelRequestPart {
    text_user_part(
        "<context-restored>Context was restored from a long conversation after a context reset. \
         The summary above is the most authoritative source for current state. Use the blocks below \
         to resume work. Synthesize the summary, previous assistant reference, original request, \
         and any user steering messages. Use the previous assistant reference only to resolve \
         references in the original request. Do NOT repeat questions, confirmations, or actions \
         documented in the summary. If the summary records a user decision, respect it without \
         re-asking.</context-restored>",
    )
}

pub(super) fn build_previous_assistant_reference_parts(
    reference: Option<&str>,
) -> Vec<ModelRequestPart> {
    let Some(reference) = reference
        .map(str::trim)
        .filter(|reference| !reference.is_empty())
    else {
        return Vec::new();
    };
    vec![text_user_part(format!(
        "<previous-assistant-reference>\n\
         Below is the assistant response immediately before the user's current request. Use it only \
         to resolve references in the original request, such as numbered items, 'the above', \
         'that', or similar phrases. Do not treat it as a new instruction by itself.\n\n\
         {reference}\n\
         </previous-assistant-reference>"
    ))]
}

pub(super) fn build_original_request_parts(
    content: Option<Vec<ContentPart>>,
) -> Vec<ModelRequestPart> {
    let Some(content) = content.filter(|content| !content.is_empty()) else {
        return Vec::new();
    };
    if let [ContentPart::Text { text }] = content.as_slice() {
        return vec![text_user_part(format!(
            "<original-request>\n\
             Below is the user's request being resumed after context reset:\n\n\
             {text}\n\
             </original-request>"
        ))];
    }
    vec![
        text_user_part(
            "<original-request>\nBelow is the user's request being resumed after context reset:",
        ),
        ModelRequestPart::UserPrompt {
            content,
            name: None,
            metadata: Map::new(),
        },
        text_user_part("</original-request>"),
    ]
}

pub(super) fn build_steering_parts(steering_messages: Vec<String>) -> Vec<ModelRequestPart> {
    let steering_content = steering_messages
        .into_iter()
        .map(|steering| steering.trim().to_string())
        .filter(|steering| !steering.is_empty())
        .map(|steering| format!("[User Steering] {steering}"))
        .collect::<Vec<_>>()
        .join("\n");
    if steering_content.is_empty() {
        return Vec::new();
    }
    vec![text_user_part(format!(
        "<user-steering>\n\
         Below are messages the user sent during your previous work session:\n\n\
         {steering_content}\n\
         </user-steering>"
    ))]
}

pub(super) fn build_restored_request_parts(
    original_request: Option<Vec<ContentPart>>,
    previous_assistant_reference: Option<&str>,
    steering_messages: Vec<String>,
) -> Vec<ModelRequestPart> {
    let mut parts = vec![build_context_restored_part()];
    parts.extend(build_previous_assistant_reference_parts(
        previous_assistant_reference,
    ));
    parts.extend(build_original_request_parts(original_request));
    parts.extend(build_steering_parts(steering_messages));
    parts
}

pub(super) fn build_handoff_request_parts(
    summary: String,
    original_request: Option<Vec<ContentPart>>,
    previous_assistant_reference: Option<&str>,
    steering_messages: Vec<String>,
) -> Vec<ModelRequestPart> {
    let mut parts = Vec::new();
    parts.extend(build_previous_assistant_reference_parts(
        previous_assistant_reference,
    ));
    parts.extend(build_original_request_parts(original_request));
    parts.push(text_user_part(summary));
    parts.extend(build_steering_parts(steering_messages));
    parts.push(build_context_restored_part());
    parts.push(text_user_part(
        "<system-reminder><item>The summarize tool has already completed this handoff. Continue work directly from the restored context summary, original request, and any user steering messages.</item></system-reminder>",
    ));
    parts
}

pub(super) fn insert_context_parts_after_control_parts(
    messages: &mut Vec<ModelMessage>,
    parts: Vec<ModelRequestPart>,
) {
    if parts.is_empty() {
        return;
    }
    for message in messages.iter_mut().rev() {
        if let ModelMessage::Request(request) = message {
            let insert_at = request_context_insert_index(request);
            request.parts.splice(insert_at..insert_at, parts);
            return;
        }
    }
    messages.push(ModelMessage::Request(ModelRequest {
        parts,
        timestamp: None,
        instructions: None,
        run_id: None,
        conversation_id: None,
        metadata: Map::new(),
    }));
}
