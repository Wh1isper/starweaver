//! Model capability media filtering.

use serde_json::json;
use starweaver_context::AgentContext;
use starweaver_model::{ContentPart, ModelMessage, ModelRequestPart};
use starweaver_runtime::AgentRunState;

use crate::filters::message::request_metadata_mut;

use super::policy::{content_policy_reason, media_policy_from_state_and_context};

pub(in crate::filters) fn capability_filter(
    state: &AgentRunState,
    context: &AgentContext,
    mut messages: Vec<ModelMessage>,
) -> Vec<ModelMessage> {
    let policy = media_policy_from_state_and_context(state, context);
    let mut replaced = 0usize;
    for message in &mut messages {
        if let ModelMessage::Request(request) = message {
            for part in &mut request.parts {
                if let ModelRequestPart::UserPrompt { content, .. } = part {
                    for item in content {
                        if let Some(reason) = content_policy_reason(item, &policy) {
                            *item = ContentPart::Text {
                                text: format!(
                                    "System reminder: media part was removed by capability policy: {reason}."
                                ),
                            };
                            replaced += 1;
                        }
                    }
                }
            }
        }
    }
    if replaced > 0 {
        request_metadata_mut(&mut messages).insert(
            "starweaver_capability_replacements".to_string(),
            json!(replaced),
        );
    }
    messages
}
