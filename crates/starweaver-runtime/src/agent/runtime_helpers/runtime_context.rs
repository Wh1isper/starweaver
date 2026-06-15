//! Runtime-owned context instruction capability.

use std::sync::Arc;

use async_trait::async_trait;
use starweaver_context::AgentContext;
use starweaver_model::{
    ContentPart, ModelMessage, ModelRequest, ModelRequestPart, INSTRUCTION_ORIGIN_METADATA,
    INSTRUCTION_ORIGIN_RUNTIME_CONTEXT,
};

use crate::{
    agent::runtime_helpers::request_instruction_end_index,
    capability::{
        AgentCapability, CapabilityResult, CapabilitySpec, RUNTIME_CONTEXT_CAPABILITY_ID,
    },
    run::AgentRunState,
};

/// Runtime-owned capability that appends current `AgentContext` state to canonical history.
#[derive(Clone, Debug, Default)]
pub(in crate::agent) struct RuntimeContextCapability;

/// Return the built-in runtime context capability.
#[must_use]
pub(in crate::agent) fn runtime_context_capability() -> Arc<dyn AgentCapability> {
    Arc::new(RuntimeContextCapability)
}

#[async_trait]
impl AgentCapability for RuntimeContextCapability {
    fn spec(&self) -> CapabilitySpec {
        CapabilitySpec::new(RUNTIME_CONTEXT_CAPABILITY_ID).with_description(
            "Injects runtime-owned AgentContext instructions into canonical model-message history.",
        )
    }

    async fn prepare_model_messages_with_context(
        &self,
        _state: &mut AgentRunState,
        context: &mut AgentContext,
        mut messages: Vec<ModelMessage>,
    ) -> CapabilityResult<Vec<ModelMessage>> {
        inject_runtime_context(context, &mut messages);
        Ok(messages)
    }
}

fn inject_runtime_context(context: &AgentContext, messages: &mut Vec<ModelMessage>) {
    let is_user_prompt = latest_request(messages)
        .map_or(true, |request| !request_has_tool_return_or_retry(request))
        || metadata_bool(&context.metadata, "starweaver_force_inject_instructions");
    let Some(text) = context.inject_runtime_context(is_user_prompt) else {
        return;
    };
    let mut metadata = serde_json::Map::new();
    metadata.insert(
        INSTRUCTION_ORIGIN_METADATA.to_string(),
        serde_json::json!(INSTRUCTION_ORIGIN_RUNTIME_CONTEXT),
    );
    insert_context_into_latest_request(
        messages,
        ModelRequestPart::UserPrompt {
            content: vec![ContentPart::Text { text }],
            name: None,
            metadata,
        },
    );
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

fn insert_context_into_latest_request(messages: &mut Vec<ModelMessage>, part: ModelRequestPart) {
    for message in messages.iter_mut().rev() {
        if let ModelMessage::Request(request) = message {
            insert_context_part_after_control_parts(request, part);
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

fn insert_context_part_after_control_parts(request: &mut ModelRequest, part: ModelRequestPart) {
    let instruction_end = request_instruction_end_index(request);
    let context_prefix_len = request.parts[instruction_end..]
        .iter()
        .take_while(|part| is_context_user_prompt(part))
        .count();
    request
        .parts
        .insert(instruction_end + context_prefix_len, part);
}

fn is_context_user_prompt(part: &ModelRequestPart) -> bool {
    match part {
        ModelRequestPart::UserPrompt { metadata, .. } => {
            metadata.contains_key(INSTRUCTION_ORIGIN_METADATA)
        }
        ModelRequestPart::SystemPrompt { .. }
        | ModelRequestPart::Instruction { .. }
        | ModelRequestPart::ToolReturn(_)
        | ModelRequestPart::RetryPrompt { .. } => false,
    }
}
