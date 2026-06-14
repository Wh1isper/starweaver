use serde_json::Map;
use starweaver_model::{ContentPart, ModelMessage, ModelRequest, ModelRequestPart};

use super::constants::{CACHE_FRIENDLY_COMPACT_INSTRUCTION, CACHE_FRIENDLY_COMPACT_PROMPT};
use super::messages::trim_message_for_compact;

pub(super) fn build_compact_summary_request(messages: &[ModelMessage]) -> Vec<ModelMessage> {
    let mut compact_messages = messages
        .iter()
        .filter_map(|message| trim_message_for_compact(message.clone()))
        .collect::<Vec<_>>();
    compact_messages.push(ModelMessage::Request(ModelRequest {
        parts: vec![
            ModelRequestPart::SystemPrompt {
                text: CACHE_FRIENDLY_COMPACT_INSTRUCTION.to_string(),
                metadata: Map::new(),
            },
            ModelRequestPart::UserPrompt {
                content: vec![ContentPart::Text {
                    text: CACHE_FRIENDLY_COMPACT_PROMPT.to_string(),
                }],
                name: None,
                metadata: Map::new(),
            },
        ],
        timestamp: None,
        instructions: None,
        run_id: None,
        conversation_id: None,
        metadata: Map::new(),
    }));
    compact_messages
}
