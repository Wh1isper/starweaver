//! Steering message helpers.

use starweaver_context::{AgentContext, AgentEvent, BusMessage};
use starweaver_model::{ContentPart, ModelRequest, ModelRequestPart};

use crate::agent::Agent;

const STEERING_GUARD_PROMPT: &str = "<system-reminder>There are pending steering messages. Continue and incorporate them before finalizing.</system-reminder>";

pub(super) fn is_steering_guard_prompt(prompt: &str) -> bool {
    prompt == STEERING_GUARD_PROMPT
}

fn is_steering_message(message: &BusMessage) -> bool {
    message
        .metadata
        .get("starweaver.topic")
        .and_then(serde_json::Value::as_str)
        == Some("steering")
}

struct SteeringMessage {
    id: Option<String>,
    text: String,
}

fn steering_message(message: &BusMessage) -> Option<SteeringMessage> {
    if !is_steering_message(message) {
        return None;
    }
    let text = message.render_text().trim().to_string();
    if text.is_empty() {
        return None;
    }
    let id = message
        .content
        .get("id")
        .or_else(|| message.content.get("message_id"))
        .and_then(serde_json::Value::as_str)
        .map_or_else(|| Some(message.id.clone()), |id| Some(id.to_string()));
    Some(SteeringMessage { id, text })
}

impl Agent {
    pub(in crate::agent) fn apply_runtime_steering_messages(
        context: &mut AgentContext,
        request: &mut ModelRequest,
    ) {
        context.subscribe_messages();
        let steering_messages = context
            .consume_messages_matching(is_steering_message)
            .into_iter()
            .filter_map(|message| steering_message(&message))
            .collect::<Vec<_>>();
        for steering in &steering_messages {
            context.publish_event(AgentEvent::new(
                "steering_received",
                serde_json::json!({"id": steering.id, "text": steering.text}),
            ));
            context.steering_messages.push(steering.text.clone());
        }
        request
            .parts
            .extend(steering_messages.into_iter().map(|steering| {
                let mut metadata = serde_json::Map::new();
                metadata.insert(
                    "starweaver.topic".to_string(),
                    serde_json::json!("steering"),
                );
                if let Some(id) = &steering.id {
                    metadata.insert("starweaver.steering_id".to_string(), serde_json::json!(id));
                }
                ModelRequestPart::UserPrompt {
                    content: vec![ContentPart::Text {
                        text: format!("Steering update from the user:\n{}", steering.text),
                    }],
                    name: Some("steering".to_string()),
                    metadata,
                }
            }));
    }

    pub(in crate::agent) fn has_pending_steering_messages(context: &AgentContext) -> bool {
        context
            .messages
            .peek(context.agent_id.as_str())
            .iter()
            .any(is_steering_message)
    }

    pub(in crate::agent) fn pending_steering_guard_message(
        context: &AgentContext,
    ) -> Option<String> {
        if Self::has_pending_steering_messages(context) {
            Some(STEERING_GUARD_PROMPT.to_string())
        } else {
            None
        }
    }
}
