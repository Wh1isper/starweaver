use serde_json::{json, Value};

use crate::{
    message::{Metadata, ModelMessage, ModelRequestPart},
    providers::is_dynamic_system_instruction,
};

pub(super) fn collect_static_openai_instructions(messages: &[ModelMessage]) -> Vec<String> {
    let mut instructions = Vec::new();
    for message in messages {
        let ModelMessage::Request(request) = message else {
            continue;
        };
        if let Some(request_instructions) = request.instructions.as_ref() {
            push_unique_instruction(&mut instructions, request_instructions);
        }
        for part in &request.parts {
            match part {
                ModelRequestPart::SystemPrompt { text, .. } => {
                    push_unique_instruction(&mut instructions, text);
                }
                ModelRequestPart::Instruction { text, metadata }
                    if !is_dynamic_instruction(metadata) =>
                {
                    push_unique_instruction(&mut instructions, text);
                }
                ModelRequestPart::Instruction { .. }
                | ModelRequestPart::UserPrompt { .. }
                | ModelRequestPart::ToolReturn(_)
                | ModelRequestPart::RetryPrompt { .. } => {}
            }
        }
    }
    instructions
}

fn push_unique_instruction(instructions: &mut Vec<String>, text: &str) {
    if text.trim().is_empty() || instructions.iter().any(|existing| existing == text) {
        return;
    }
    instructions.push(text.to_string());
}

pub(super) fn is_dynamic_instruction(metadata: &Metadata) -> bool {
    is_dynamic_system_instruction(metadata)
}

pub(super) fn push_instruction_message(text: &str, input: &mut Vec<Value>) {
    if text.trim().is_empty() {
        return;
    }
    input.push(json!({
        "role": "system",
        "content": [{"type": "input_text", "text": text}],
    }));
}
