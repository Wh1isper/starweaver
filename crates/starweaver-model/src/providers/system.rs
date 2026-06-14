//! Provider system instruction collection helpers.

use serde_json::Value;

use crate::message::{Metadata, ModelMessage, ModelRequestPart};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SystemInstructionPart {
    pub(crate) text: String,
    pub(crate) dynamic: bool,
}

impl SystemInstructionPart {
    const fn request_level(text: String) -> Self {
        Self {
            text,
            dynamic: false,
        }
    }

    const fn system_prompt(text: String) -> Self {
        Self {
            text,
            dynamic: false,
        }
    }

    fn instruction(text: String, metadata: &Metadata) -> Self {
        Self {
            text,
            dynamic: is_dynamic_system_instruction(metadata),
        }
    }
}

pub fn is_dynamic_system_instruction(metadata: &Metadata) -> bool {
    metadata
        .get("starweaver_instruction_dynamic")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || metadata
            .get("starweaver_instruction_origin")
            .and_then(Value::as_str)
            .is_some_and(|origin| matches!(origin, "runtime_context" | "environment_context"))
}

pub fn collect_system_parts_and_non_system(
    messages: &[ModelMessage],
) -> (Vec<SystemInstructionPart>, Vec<&ModelMessage>) {
    let mut system = Vec::new();
    let mut rest = Vec::new();

    for message in messages {
        match message {
            ModelMessage::Request(request) => {
                if let Some(request_instructions) = request.instructions.as_ref() {
                    if !request_instructions.trim().is_empty() {
                        system.push(SystemInstructionPart::request_level(
                            request_instructions.clone(),
                        ));
                    }
                }
                let mut has_non_system = false;
                for part in &request.parts {
                    match part {
                        ModelRequestPart::SystemPrompt { text, .. } => {
                            system.push(SystemInstructionPart::system_prompt(text.clone()));
                        }
                        ModelRequestPart::Instruction { text, metadata } => {
                            system.push(SystemInstructionPart::instruction(text.clone(), metadata));
                        }
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

pub fn collect_system_and_non_system(
    messages: &[ModelMessage],
) -> (Vec<String>, Vec<&ModelMessage>) {
    let (system_parts, rest) = collect_system_parts_and_non_system(messages);
    (
        system_parts.into_iter().map(|part| part.text).collect(),
        rest,
    )
}
