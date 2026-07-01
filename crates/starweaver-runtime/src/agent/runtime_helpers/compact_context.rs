//! Compact restore context metadata helpers.

use serde_json::{Map, Value};
use starweaver_context::AgentContext;
use starweaver_model::{ContentPart, ModelRequest, ModelRequestPart, context_origin_metadata};

use crate::run::AgentRunState;

use super::request_parts::request_instruction_end_index;

pub(in crate::agent) const ORIGINAL_REQUEST_METADATA: &str = "starweaver_original_request";
pub(in crate::agent) const ORIGINAL_REQUEST_CONTENT_METADATA: &str =
    "starweaver_original_request_content";
pub(in crate::agent) const PREVIOUS_ASSISTANT_RESPONSE_REFERENCE_METADATA: &str =
    "starweaver_previous_assistant_response_reference";
pub(in crate::agent) const USER_STEERING_METADATA: &str = "starweaver_user_steering";

impl crate::agent::Agent {
    pub(in crate::agent) fn capture_effective_user_prompt_for_compact_restore(
        context: &mut AgentContext,
        request: &ModelRequest,
    ) {
        if let Some(content) = effective_user_prompt_content(request) {
            context.user_prompts = Some(content);
        }
    }

    pub(in crate::agent) fn sync_compact_context_metadata(
        context: &AgentContext,
        state: &mut AgentRunState,
    ) {
        if let Some(user_prompts) = &context.user_prompts
            && !user_prompts.is_empty()
        {
            if let Ok(value) = serde_json::to_value(user_prompts) {
                state
                    .metadata
                    .insert(ORIGINAL_REQUEST_CONTENT_METADATA.to_string(), value);
            }
            if let Some(text) = user_prompt_text(user_prompts) {
                state.metadata.insert(
                    ORIGINAL_REQUEST_METADATA.to_string(),
                    serde_json::json!(text),
                );
            }
        }
        if let Some(reference) = &context.previous_assistant_response_reference {
            state.metadata.insert(
                PREVIOUS_ASSISTANT_RESPONSE_REFERENCE_METADATA.to_string(),
                serde_json::json!(reference),
            );
        }
        if !context.steering_messages.is_empty() {
            state.metadata.insert(
                USER_STEERING_METADATA.to_string(),
                serde_json::json!(context.steering_messages),
            );
        }
    }
}

fn user_prompt_text(content: &[ContentPart]) -> Option<String> {
    let text = content
        .iter()
        .filter_map(|part| match part {
            ContentPart::Text { text } => Some(text.trim()),
            ContentPart::ImageUrl { .. }
            | ContentPart::FileUrl { .. }
            | ContentPart::Binary { .. }
            | ContentPart::ResourceRef { .. }
            | ContentPart::DataUrl { .. } => None,
        })
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    (!text.is_empty()).then_some(text)
}

fn effective_user_prompt_content(request: &ModelRequest) -> Option<Vec<ContentPart>> {
    let instruction_end = request_instruction_end_index(request);
    request.parts[instruction_end..]
        .iter()
        .find_map(|part| match part {
            ModelRequestPart::UserPrompt {
                content, metadata, ..
            } if !content.is_empty() && !is_context_user_prompt(metadata) => Some(content.clone()),
            ModelRequestPart::UserPrompt { .. }
            | ModelRequestPart::SystemPrompt { .. }
            | ModelRequestPart::Instruction { .. }
            | ModelRequestPart::ToolReturn(_)
            | ModelRequestPart::RetryPrompt { .. } => None,
        })
}

fn is_context_user_prompt(metadata: &Map<String, Value>) -> bool {
    context_origin_metadata(metadata).is_some()
}
