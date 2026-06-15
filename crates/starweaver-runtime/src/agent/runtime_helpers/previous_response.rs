//! Previous visible assistant response helpers.

use starweaver_model::ModelMessage;

use crate::agent::Agent;

const PREVIOUS_ASSISTANT_RESPONSE_REFERENCE_MAX_CHARS: usize = 32_000;
const PREVIOUS_ASSISTANT_RESPONSE_REFERENCE_KEEP_HEAD: usize = 24_000;
const PREVIOUS_ASSISTANT_RESPONSE_REFERENCE_KEEP_TAIL: usize = 6_000;

impl Agent {
    pub(in crate::agent) fn previous_assistant_response_reference(
        message_history: &[ModelMessage],
    ) -> Option<String> {
        previous_assistant_response_reference(message_history)
    }
}

fn previous_assistant_response_reference(message_history: &[ModelMessage]) -> Option<String> {
    for message in message_history.iter().rev() {
        let ModelMessage::Response(response) = message else {
            continue;
        };
        let chunks = response
            .parts
            .iter()
            .filter_map(starweaver_model::ModelResponsePart::text)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>();
        if !chunks.is_empty() {
            return truncate_previous_assistant_response_reference(&chunks.join("\n\n"));
        }
    }
    None
}

fn truncate_previous_assistant_response_reference(text: &str) -> Option<String> {
    let stripped = text.trim();
    if stripped.is_empty() {
        return None;
    }
    let total = stripped.chars().count();
    if total <= PREVIOUS_ASSISTANT_RESPONSE_REFERENCE_MAX_CHARS {
        return Some(stripped.to_string());
    }

    let head = stripped
        .chars()
        .take(PREVIOUS_ASSISTANT_RESPONSE_REFERENCE_KEEP_HEAD)
        .collect::<String>();
    let tail = stripped
        .chars()
        .rev()
        .take(PREVIOUS_ASSISTANT_RESPONSE_REFERENCE_KEEP_TAIL)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    let truncated = total.saturating_sub(
        PREVIOUS_ASSISTANT_RESPONSE_REFERENCE_KEEP_HEAD
            + PREVIOUS_ASSISTANT_RESPONSE_REFERENCE_KEEP_TAIL,
    );
    Some(format!(
        "{head}\n[... {truncated} chars truncated from previous assistant response ...]\n{tail}"
    ))
}

#[cfg(test)]
mod tests {
    use serde_json::Map;
    use starweaver_core::Usage;
    use starweaver_model::{ModelRequest, ModelResponse, ModelResponsePart, ToolCallPart};

    use super::*;

    #[test]
    fn extracts_latest_visible_assistant_text() {
        let history = vec![
            ModelMessage::Request(ModelRequest::user_text("What should we do?")),
            ModelMessage::Response(ModelResponse::text("1. Add tests\n2. Update docs")),
            ModelMessage::Request(ModelRequest::user_text("1 and 2")),
        ];

        assert_eq!(
            previous_assistant_response_reference(&history).as_deref(),
            Some("1. Add tests\n2. Update docs"),
        );
    }

    #[test]
    fn skips_non_visible_assistant_parts() {
        let history = vec![
            ModelMessage::Response(ModelResponse::text("visible answer")),
            ModelMessage::Response(ModelResponse {
                parts: vec![
                    ModelResponsePart::Thinking {
                        text: "private reasoning".to_string(),
                        signature: None,
                    },
                    ModelResponsePart::ToolCall(ToolCallPart {
                        id: "tool-1".to_string(),
                        name: "shell".to_string(),
                        arguments: serde_json::json!({"command": "echo hi"}).into(),
                    }),
                ],
                usage: Usage::default(),
                model_name: None,
                provider: None,
                finish_reason: None,
                timestamp: None,
                run_id: None,
                conversation_id: None,
                metadata: Map::new(),
            }),
        ];

        assert_eq!(
            previous_assistant_response_reference(&history).as_deref(),
            Some("visible answer"),
        );
    }

    #[test]
    fn bounds_long_visible_text() {
        let text = format!("{}{}", "H".repeat(26_000), "T".repeat(10_000));
        let Some(truncated) = truncate_previous_assistant_response_reference(&text) else {
            panic!("long non-empty text should produce a truncated reference");
        };

        assert!(truncated.len() < text.len());
        assert!(truncated.contains("chars truncated from previous assistant response"));
        assert!(truncated.starts_with(&"H".repeat(100)));
        assert!(truncated.ends_with(&"T".repeat(100)));
    }
}
