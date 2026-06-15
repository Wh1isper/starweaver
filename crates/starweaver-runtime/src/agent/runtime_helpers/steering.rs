//! Steering message helpers.

use starweaver_context::{AgentContext, AgentEvent, BusMessage};
use starweaver_model::{ContentPart, ModelRequest, ModelRequestPart};

use crate::agent::Agent;

const STEERING_GUARD_PROMPT: &str = "<system-reminder>There are pending steering messages. Continue and incorporate them before finalizing.</system-reminder>";

struct SteeringMessage {
    id: Option<String>,
    text: String,
}

pub(super) fn is_steering_guard_prompt(prompt: &str) -> bool {
    prompt == STEERING_GUARD_PROMPT
}

fn steering_message(message: &BusMessage) -> Option<SteeringMessage> {
    if message.topic != "steering" {
        return None;
    }
    let text = message
        .payload
        .get("text")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())?
        .to_string();
    let id = message
        .payload
        .get("id")
        .or_else(|| message.payload.get("message_id"))
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string);
    Some(SteeringMessage { id, text })
}

impl Agent {
    pub(in crate::agent) fn apply_steering_messages(
        context: &mut AgentContext,
        request: &mut ModelRequest,
    ) {
        let mut steering_messages = Vec::new();
        let mut retained_messages = Vec::new();
        while let Some(message) = context.messages.dequeue() {
            if let Some(steering) = steering_message(&message) {
                context.publish_event(AgentEvent::new(
                    "steering_received",
                    serde_json::json!({"id": steering.id, "text": steering.text}),
                ));
                context.steering_messages.push(steering.text.clone());
                steering_messages.push(steering);
            } else {
                retained_messages.push(message);
            }
        }
        for message in retained_messages {
            context.enqueue_message(message);
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
        context.messages.has_topic("steering")
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
