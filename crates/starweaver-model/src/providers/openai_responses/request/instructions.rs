use crate::{message::ModelMessage, providers::collect_system_parts_and_non_system};

pub(super) fn collect_openai_instructions(messages: &[ModelMessage]) -> Vec<String> {
    let (system_parts, _) = collect_system_parts_and_non_system(messages);
    let mut instructions = Vec::new();
    for part in system_parts {
        push_unique_instruction(&mut instructions, &part.text);
    }
    instructions
}

fn push_unique_instruction(instructions: &mut Vec<String>, text: &str) {
    if text.trim().is_empty() || instructions.iter().any(|existing| existing == text) {
        return;
    }
    instructions.push(text.to_string());
}
