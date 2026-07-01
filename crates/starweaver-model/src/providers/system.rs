//! Provider system instruction collection helpers.

use serde_json::Value;

use crate::{
    message::{Metadata, ModelMessage, ModelRequest, ModelRequestPart},
    request::{current_instruction_request_index, is_dynamic_instruction_metadata},
};

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
    is_dynamic_instruction_metadata(metadata)
}

pub fn collect_system_parts_and_non_system(
    messages: &[ModelMessage],
) -> (Vec<SystemInstructionPart>, Vec<&ModelMessage>) {
    let mut system = Vec::new();
    let mut rest = Vec::new();
    let current_instruction_index = current_instruction_request_index(messages);

    for (index, message) in messages.iter().enumerate() {
        match message {
            ModelMessage::Request(request) => {
                let collect_instruction_material =
                    current_instruction_index == Some(index) || is_lifted_system_request(request);
                if collect_instruction_material
                    && let Some(request_instructions) = request.instructions.as_ref()
                    && !request_instructions.trim().is_empty()
                {
                    system.push(SystemInstructionPart::request_level(
                        request_instructions.clone(),
                    ));
                }
                let mut has_non_system = false;
                for part in &request.parts {
                    match part {
                        ModelRequestPart::SystemPrompt { text, .. } => {
                            system.push(SystemInstructionPart::system_prompt(text.clone()));
                        }
                        ModelRequestPart::Instruction { text, metadata }
                            if collect_instruction_material =>
                        {
                            system.push(SystemInstructionPart::instruction(text.clone(), metadata));
                        }
                        ModelRequestPart::Instruction { .. } => {}
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

fn is_lifted_system_request(request: &ModelRequest) -> bool {
    request
        .metadata
        .get("starweaver_instruction_origin")
        .and_then(Value::as_str)
        .is_some_and(|origin| origin == "lifted_system")
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
