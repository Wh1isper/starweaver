use serde_json::Map;

use crate::{
    message::{ModelMessage, ModelRequest, ModelRequestPart},
    profile::MessageNormalization,
    request::current_instruction_request_index,
};

/// Normalize canonical history according to a provider profile policy.
#[must_use]
pub fn prepare_messages(
    messages: &[ModelMessage],
    normalization: MessageNormalization,
) -> Vec<ModelMessage> {
    match normalization {
        MessageNormalization::PreserveItems => messages.to_vec(),
        MessageNormalization::MergeAdjacentSameRole => merge_adjacent_requests(messages),
        MessageNormalization::SystemField | MessageNormalization::SystemInstruction => {
            lift_system_parts(messages)
        }
        MessageNormalization::WrapInlineSystem => wrap_inline_system_parts(messages),
    }
}

fn merge_adjacent_requests(messages: &[ModelMessage]) -> Vec<ModelMessage> {
    let current_instruction_index = current_instruction_request_index(messages);
    let mut output = Vec::new();
    for (index, message) in messages.iter().enumerate() {
        let message = match message {
            ModelMessage::Request(request) => ModelMessage::Request(request_for_normalization(
                request,
                current_instruction_index == Some(index),
            )),
            ModelMessage::Response(_) => message.clone(),
        };
        if let (Some(ModelMessage::Request(previous)), ModelMessage::Request(next)) =
            (output.last_mut(), &message)
        {
            previous.parts.extend(next.parts.clone());
            previous.metadata.extend(next.metadata.clone());
            previous.instructions = merge_optional_instructions(
                previous.instructions.take(),
                next.instructions.clone(),
            );
        } else {
            output.push(message);
        }
    }
    output
}

fn request_for_normalization(
    request: &ModelRequest,
    keep_instruction_material: bool,
) -> ModelRequest {
    if keep_instruction_material {
        return request.clone();
    }
    let mut request = request.clone();
    request.instructions = None;
    request
        .parts
        .retain(|part| !matches!(part, ModelRequestPart::Instruction { .. }));
    request
}

fn merge_optional_instructions(left: Option<String>, right: Option<String>) -> Option<String> {
    match (left, right) {
        (Some(left), Some(right)) if !left.trim().is_empty() && !right.trim().is_empty() => {
            Some(format!(
                "{left}

{right}"
            ))
        }
        (Some(left), _) if !left.trim().is_empty() => Some(left),
        (_, Some(right)) if !right.trim().is_empty() => Some(right),
        _ => None,
    }
}

fn lift_system_parts(messages: &[ModelMessage]) -> Vec<ModelMessage> {
    let mut lifted = Vec::new();
    let mut output = Vec::new();
    let current_instruction_index = current_instruction_request_index(messages);
    for (index, message) in messages.iter().enumerate() {
        match message {
            ModelMessage::Request(request) => {
                let collect_instruction_material = current_instruction_index == Some(index);
                if collect_instruction_material
                    && let Some(instructions) = request.instructions.as_ref()
                    && !instructions.trim().is_empty()
                {
                    let mut metadata = Map::new();
                    metadata.insert(
                        "starweaver_instruction_origin".to_string(),
                        serde_json::json!("request_instructions"),
                    );
                    lifted.push(ModelRequestPart::SystemPrompt {
                        text: instructions.clone(),
                        metadata,
                    });
                }
                let mut remaining = Vec::new();
                for part in &request.parts {
                    match part {
                        ModelRequestPart::SystemPrompt { .. } => lifted.push(part.clone()),
                        ModelRequestPart::Instruction { .. } if collect_instruction_material => {
                            lifted.push(part.clone());
                        }
                        ModelRequestPart::Instruction { .. } => {}
                        other => remaining.push(other.clone()),
                    }
                }
                if !remaining.is_empty() {
                    let mut request = request.clone();
                    request.parts = remaining;
                    request.instructions = None;
                    output.push(ModelMessage::Request(request));
                }
            }
            ModelMessage::Response(_) => output.push(message.clone()),
        }
    }
    if lifted.is_empty() {
        return output;
    }
    output.insert(
        0,
        ModelMessage::Request(ModelRequest {
            parts: lifted,
            timestamp: None,
            instructions: None,
            run_id: None,
            conversation_id: None,
            metadata: Map::from_iter([(
                "starweaver_instruction_origin".to_string(),
                serde_json::json!("lifted_system"),
            )]),
        }),
    );
    output
}

fn wrap_inline_system_parts(messages: &[ModelMessage]) -> Vec<ModelMessage> {
    let current_instruction_index = current_instruction_request_index(messages);
    messages
        .iter()
        .enumerate()
        .map(|(index, message)| match message {
            ModelMessage::Request(request) => {
                let collect_instruction_material = current_instruction_index == Some(index);
                let mut request = request.clone();
                let request_level_instruction = collect_instruction_material
                    .then(|| request.instructions.take())
                    .flatten()
                    .map(|text| ModelRequestPart::UserPrompt {
                        content: vec![crate::message::ContentPart::Text {
                            text: format!("<system>{text}</system>"),
                        }],
                        name: None,
                        metadata: Map::new(),
                    });
                request.parts = request_level_instruction
                    .into_iter()
                    .chain(request.parts.into_iter().filter_map(|part| match part {
                        ModelRequestPart::SystemPrompt { text, metadata } => {
                            Some(ModelRequestPart::UserPrompt {
                                content: vec![crate::message::ContentPart::Text {
                                    text: format!("<system>{text}</system>"),
                                }],
                                name: None,
                                metadata,
                            })
                        }
                        ModelRequestPart::Instruction { text, metadata }
                            if collect_instruction_material =>
                        {
                            Some(ModelRequestPart::UserPrompt {
                                content: vec![crate::message::ContentPart::Text {
                                    text: format!("<system>{text}</system>"),
                                }],
                                name: None,
                                metadata,
                            })
                        }
                        ModelRequestPart::Instruction { .. } => None,
                        other => Some(other),
                    }))
                    .collect();
                ModelMessage::Request(request)
            }
            ModelMessage::Response(_) => message.clone(),
        })
        .collect()
}
