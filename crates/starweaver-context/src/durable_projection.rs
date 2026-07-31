//! Narrow projection of process-local model evidence out of durable state.

use starweaver_model::{
    CONTEXT_ORIGIN_METADATA, CONTEXT_ORIGIN_TOOL_RETURN_MEDIA, ContentPart, ModelMessage,
    ModelRequestPart, ToolReturnPart,
};

use crate::{AgentRunState, ResumableState};

const GEOMETRY_BOUND_MEDIA_METADATA: &str = "starweaver_geometry_bound_immutable_media";
const TOOL_RETURN_CONTENT_PARTS_METADATA: &str = "starweaver_tool_return_content_parts";

/// Remove only process-local Computer Use screenshot bytes from a durable tool return.
///
/// The geometry-bound marker is emitted by the Computer Use bundle and is the existing precise
/// discriminator for its immutable screenshot evidence. Prompt, marker, and unrelated private
/// metadata remain available as durable audit/context evidence. Live tool returns must not be
/// passed to this function; callers project a clone at their durable-evidence boundary.
pub fn project_tool_return_for_durable_evidence(tool_return: &mut ToolReturnPart) {
    if is_geometry_bound(&tool_return.private_metadata) {
        tool_return
            .private_metadata
            .remove(TOOL_RETURN_CONTENT_PARTS_METADATA);
    }
}

pub fn project_run_state(state: &AgentRunState) -> AgentRunState {
    let mut projected = state.clone();
    project_messages(&mut projected.message_history);
    project_tool_returns(&mut projected.pending_tool_returns);
    project_tool_returns(&mut projected.pending_approval_tool_returns);
    project_tool_returns(&mut projected.deferred_tool_returns);
    projected
}

pub fn project_resumable_state(state: &mut ResumableState) {
    project_messages(&mut state.message_history);
    project_tool_returns(&mut state.pending_tool_returns);
    for history in state.subagent_history.values_mut() {
        project_messages(history);
    }
}

fn project_messages(messages: &mut Vec<ModelMessage>) {
    for message in messages {
        let ModelMessage::Request(request) = message else {
            continue;
        };
        request.parts.retain_mut(|part| match part {
            ModelRequestPart::ToolReturn(tool_return) => {
                project_tool_return_for_durable_evidence(tool_return);
                true
            }
            ModelRequestPart::UserPrompt {
                content, metadata, ..
            } if is_geometry_bound(metadata)
                && metadata
                    .get(CONTEXT_ORIGIN_METADATA)
                    .and_then(serde_json::Value::as_str)
                    == Some(CONTEXT_ORIGIN_TOOL_RETURN_MEDIA) =>
            {
                // This is the runtime-generated carrier for the same process-local screenshot.
                // Drop the complete carrier rather than restore misleading "attached screenshot"
                // text without its exact geometry evidence.
                !content
                    .iter()
                    .any(|part| matches!(part, ContentPart::DataUrl { .. }))
            }
            _ => true,
        });
    }
}

fn project_tool_returns(tool_returns: &mut [ToolReturnPart]) {
    for tool_return in tool_returns {
        project_tool_return_for_durable_evidence(tool_return);
    }
}

fn is_geometry_bound(metadata: &serde_json::Map<String, serde_json::Value>) -> bool {
    metadata
        .get(GEOMETRY_BOUND_MEDIA_METADATA)
        .and_then(serde_json::Value::as_bool)
        == Some(true)
}
