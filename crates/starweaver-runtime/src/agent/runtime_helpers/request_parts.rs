//! Request part ordering helpers.

use starweaver_model::{
    context_origin_metadata, ModelRequest, ModelRequestPart, CONTEXT_ORIGIN_TOOL_RETURN_MEDIA,
    INSTRUCTION_DYNAMIC_METADATA,
};

pub(in crate::agent) fn request_control_prefix_len(request: &ModelRequest) -> usize {
    request
        .parts
        .iter()
        .take_while(|part| is_control_prefix_part(part))
        .count()
}

pub(in crate::agent) fn request_instruction_insert_index(request: &ModelRequest) -> usize {
    let control_prefix_len = request_control_prefix_len(request);
    control_prefix_len
        + request.parts[control_prefix_len..]
            .iter()
            .take_while(|part| is_static_instruction_prefix_part(part))
            .count()
}

pub(in crate::agent) fn request_instruction_end_index(request: &ModelRequest) -> usize {
    let control_prefix_len = request_control_prefix_len(request);
    control_prefix_len
        + request.parts[control_prefix_len..]
            .iter()
            .take_while(|part| is_instruction_prefix_part(part))
            .count()
}

fn is_control_prefix_part(part: &ModelRequestPart) -> bool {
    match part {
        ModelRequestPart::ToolReturn(_) | ModelRequestPart::RetryPrompt { .. } => true,
        ModelRequestPart::UserPrompt { metadata, .. } => context_origin_metadata(metadata)
            .is_some_and(|origin| origin == CONTEXT_ORIGIN_TOOL_RETURN_MEDIA),
        ModelRequestPart::SystemPrompt { .. } | ModelRequestPart::Instruction { .. } => false,
    }
}

fn is_static_instruction_prefix_part(part: &ModelRequestPart) -> bool {
    match part {
        ModelRequestPart::SystemPrompt { .. } => true,
        ModelRequestPart::Instruction { metadata, .. } => !metadata
            .get(INSTRUCTION_DYNAMIC_METADATA)
            .and_then(serde_json::Value::as_bool)
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
