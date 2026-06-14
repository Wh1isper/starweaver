//! Runtime context instruction injection helpers.

use starweaver_context::AgentContext;
use starweaver_model::{ModelMessage, ModelRequest, ModelRequestPart};

use crate::agent::Agent;

impl Agent {
    pub(in crate::agent) fn inject_runtime_context(
        context: &AgentContext,
        messages: &mut Vec<ModelMessage>,
    ) {
        let is_user_prompt = latest_request(messages)
            .map_or(true, |request| !request_has_tool_return_or_retry(request))
            || metadata_bool(&context.metadata, "starweaver_force_inject_instructions");
        let Some(text) = context.inject_runtime_context(is_user_prompt) else {
            return;
        };
        let mut metadata = serde_json::Map::new();
        metadata.insert(
            "starweaver_instruction_origin".to_string(),
            serde_json::json!("runtime_context"),
        );
        metadata.insert(
            "starweaver_instruction_dynamic".to_string(),
            serde_json::json!(true),
        );
        insert_instruction_into_latest_request(
            messages,
            ModelRequestPart::Instruction { text, metadata },
        );
    }
}
fn latest_request(messages: &[ModelMessage]) -> Option<&ModelRequest> {
    messages.iter().rev().find_map(|message| match message {
        ModelMessage::Request(request) => Some(request),
        ModelMessage::Response(_) => None,
    })
}

fn request_has_tool_return_or_retry(request: &ModelRequest) -> bool {
    request.parts.iter().any(|part| {
        matches!(
            part,
            ModelRequestPart::ToolReturn(_) | ModelRequestPart::RetryPrompt { .. }
        )
    })
}

fn metadata_bool(metadata: &serde_json::Map<String, serde_json::Value>, key: &str) -> bool {
    metadata
        .get(key)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn insert_instruction_into_latest_request(
    messages: &mut Vec<ModelMessage>,
    part: ModelRequestPart,
) {
    for message in messages.iter_mut().rev() {
        if let ModelMessage::Request(request) = message {
            insert_request_part_after_control_parts(request, part);
            return;
        }
    }
    messages.push(ModelMessage::Request(ModelRequest {
        parts: vec![part],
        timestamp: None,
        instructions: None,
        run_id: None,
        conversation_id: None,
        metadata: serde_json::Map::new(),
    }));
}

fn insert_request_part_after_control_parts(request: &mut ModelRequest, part: ModelRequestPart) {
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
}
