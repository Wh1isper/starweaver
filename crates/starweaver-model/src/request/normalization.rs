use serde_json::Map;

use crate::{
    message::{ModelMessage, ModelRequest, ModelRequestPart},
    profile::MessageNormalization,
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
    let mut output = Vec::new();
    for message in messages {
        match (output.last_mut(), message) {
            (Some(ModelMessage::Request(previous)), ModelMessage::Request(next)) => {
                previous.parts.extend(next.parts.clone());
                previous.metadata.extend(next.metadata.clone());
                previous.instructions = merge_optional_instructions(
                    previous.instructions.take(),
                    next.instructions.clone(),
                );
            }
            _ => output.push(message.clone()),
        }
    }
    output
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
    for message in messages {
        match message {
            ModelMessage::Request(request) => {
                if let Some(instructions) = request.instructions.as_ref() {
                    if !instructions.trim().is_empty() {
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
                }
                let mut remaining = Vec::new();
                for part in &request.parts {
                    match part {
                        ModelRequestPart::SystemPrompt { .. }
                        | ModelRequestPart::Instruction { .. } => lifted.push(part.clone()),
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
    messages
        .iter()
        .map(|message| match message {
            ModelMessage::Request(request) => {
                let mut request = request.clone();
                let request_level_instruction =
                    request
                        .instructions
                        .take()
                        .map(|text| ModelRequestPart::UserPrompt {
                            content: vec![crate::message::ContentPart::Text {
                                text: format!("<system>{text}</system>"),
                            }],
                            name: None,
                            metadata: Map::new(),
                        });
                request.parts = request_level_instruction
                    .into_iter()
                    .chain(request.parts.into_iter().map(|part| match part {
                        ModelRequestPart::SystemPrompt { text, metadata }
                        | ModelRequestPart::Instruction { text, metadata } => {
                            ModelRequestPart::UserPrompt {
                                content: vec![crate::message::ContentPart::Text {
                                    text: format!("<system>{text}</system>"),
                                }],
                                name: None,
                                metadata,
                            }
                        }
                        other => other,
                    }))
                    .collect();
                ModelMessage::Request(request)
            }
            ModelMessage::Response(_) => message.clone(),
        })
        .collect()
}
