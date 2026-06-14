use serde_json::json;
use starweaver_model::{ModelMessage, ModelResponsePart};

use crate::filters::message::request_metadata_mut;

pub(super) fn reasoning_normalize_filter(mut messages: Vec<ModelMessage>) -> Vec<ModelMessage> {
    let mut removed = 0usize;
    for message in &mut messages {
        if let ModelMessage::Response(response) = message {
            let before = response.parts.len();
            response.parts.retain(reasoning_part_has_replay_value);
            removed += before.saturating_sub(response.parts.len());
            for part in &mut response.parts {
                match part {
                    ModelResponsePart::Thinking { text, signature }
                    | ModelResponsePart::ProviderThinking {
                        text, signature, ..
                    } => {
                        *text = normalize_reasoning_text(text);
                        if signature.as_deref().is_some_and(str::is_empty) {
                            *signature = None;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    if removed > 0 {
        request_metadata_mut(&mut messages).insert(
            "starweaver_reasoning_removed_empty".to_string(),
            json!(removed),
        );
    }
    messages
}

fn reasoning_part_has_replay_value(part: &ModelResponsePart) -> bool {
    match part {
        ModelResponsePart::Thinking { text, signature } => {
            !text.trim().is_empty() || signature.as_deref().is_some_and(|value| !value.is_empty())
        }
        ModelResponsePart::ProviderThinking {
            text,
            signature,
            provider,
        } => {
            !text.trim().is_empty()
                || signature.as_deref().is_some_and(|value| !value.is_empty())
                || provider.id.is_some()
                || !provider.details.is_empty()
        }
        _ => true,
    }
}

fn normalize_reasoning_text(text: &str) -> String {
    text.lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}
