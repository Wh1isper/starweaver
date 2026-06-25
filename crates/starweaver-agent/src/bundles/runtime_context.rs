use async_trait::async_trait;
use starweaver_context::AgentContext;
use starweaver_model::{
    context_origin_metadata, ContentPart, ModelMessage, ModelRequest, ModelRequestPart,
    CONTEXT_ORIGIN_METADATA, CONTEXT_ORIGIN_RUNTIME_CONTEXT, CONTEXT_ORIGIN_TOOL_RETURN_MEDIA,
    CONTEXT_TYPE_METADATA,
};
use starweaver_runtime::{
    AgentCapability, AgentRunState, CapabilityResult, CapabilitySpec, RUNTIME_CONTEXT_CAPABILITY_ID,
};

/// SDK capability that appends current `AgentContext` state to provider-facing requests.
#[derive(Clone, Debug, Default)]
pub struct RuntimeContextCapability;

#[async_trait]
impl AgentCapability for RuntimeContextCapability {
    fn spec(&self) -> CapabilitySpec {
        CapabilitySpec::new(RUNTIME_CONTEXT_CAPABILITY_ID).with_description(
            "Injects SDK AgentContext runtime context into the current provider request.",
        )
    }

    async fn prepare_model_messages_with_context(
        &self,
        _state: &mut AgentRunState,
        _context: &mut AgentContext,
        messages: Vec<ModelMessage>,
    ) -> CapabilityResult<Vec<ModelMessage>> {
        Ok(messages)
    }

    async fn prepare_provider_messages_with_context(
        &self,
        _state: &mut AgentRunState,
        context: &mut AgentContext,
        mut messages: Vec<ModelMessage>,
    ) -> CapabilityResult<Vec<ModelMessage>> {
        strip_context_origin_from_requests(&mut messages, CONTEXT_ORIGIN_RUNTIME_CONTEXT);
        inject_runtime_context(context, &mut messages);
        Ok(messages)
    }
}

fn inject_runtime_context(context: &AgentContext, messages: &mut [ModelMessage]) {
    let is_user_prompt = latest_request(messages)
        .is_none_or(|request| !request_has_tool_return_or_retry(request))
        || context.force_inject_context;
    let Some(text) = context.render_runtime_context(is_user_prompt) else {
        return;
    };
    let mut metadata = serde_json::Map::new();
    metadata.insert(
        CONTEXT_TYPE_METADATA.to_string(),
        serde_json::json!("runtime"),
    );
    metadata.insert(
        CONTEXT_ORIGIN_METADATA.to_string(),
        serde_json::json!(CONTEXT_ORIGIN_RUNTIME_CONTEXT),
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

fn strip_context_origin_from_requests(messages: &mut [ModelMessage], origin: &str) {
    for message in messages {
        let ModelMessage::Request(request) = message else {
            continue;
        };
        request
            .parts
            .retain(|part| part_context_origin(part) != Some(origin));
    }
}

fn part_context_origin(part: &ModelRequestPart) -> Option<&str> {
    match part {
        ModelRequestPart::SystemPrompt { metadata, .. }
        | ModelRequestPart::UserPrompt { metadata, .. }
        | ModelRequestPart::Instruction { metadata, .. } => context_origin_metadata(metadata),
        ModelRequestPart::ToolReturn(_) | ModelRequestPart::RetryPrompt { .. } => None,
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

fn insert_context_into_latest_request(messages: &mut [ModelMessage], part: ModelRequestPart) {
    for message in messages.iter_mut().rev() {
        if let ModelMessage::Request(request) = message {
            let insert_at = request_context_insert_index(request);
            request.parts.insert(insert_at, part);
            return;
        }
    }
}

fn request_context_insert_index(request: &ModelRequest) -> usize {
    request_instruction_end_index(request)
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
        ModelRequestPart::UserPrompt { .. } => part_context_origin(part)
            .is_some_and(|origin| origin == CONTEXT_ORIGIN_TOOL_RETURN_MEDIA),
        ModelRequestPart::SystemPrompt { .. } | ModelRequestPart::Instruction { .. } => false,
    }
}

const fn is_instruction_prefix_part(part: &ModelRequestPart) -> bool {
    matches!(
        part,
        ModelRequestPart::SystemPrompt { .. } | ModelRequestPart::Instruction { .. }
    )
}
