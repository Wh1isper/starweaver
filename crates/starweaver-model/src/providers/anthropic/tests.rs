#![allow(clippy::unwrap_used)]

use serde_json::json;

use super::AnthropicMessagesAdapter;
use crate::message::{ModelMessage, ModelResponse, ModelResponsePart, ProviderPartInfo};

fn response_with_provider_thinking(provider_name: &str) -> ModelMessage {
    ModelMessage::Response(ModelResponse {
        parts: vec![ModelResponsePart::ProviderThinking {
            text: "inspect context".to_string(),
            signature: Some("provider-signature".to_string()),
            provider: ProviderPartInfo::new(provider_name).with_id("thinking_1"),
        }],
        usage: starweaver_usage::Usage::default(),
        model_name: None,
        provider: None,
        finish_reason: None,
        timestamp: None,
        run_id: None,
        conversation_id: None,
        metadata: serde_json::Map::new(),
    })
}

#[test]
fn build_request_replays_anthropic_provider_thinking_natively() {
    let request = AnthropicMessagesAdapter::build_request(
        "claude-test",
        &[response_with_provider_thinking("anthropic")],
        None,
        &[],
    )
    .unwrap();

    let content = request["messages"][0]["content"].as_array().unwrap();
    assert_eq!(content[0]["type"], "thinking");
    assert_eq!(content[0]["thinking"], "inspect context");
    assert_eq!(content[0]["signature"], "provider-signature");
}

#[test]
fn build_request_does_not_replay_foreign_thinking_signature() {
    let request = AnthropicMessagesAdapter::build_request(
        "claude-test",
        &[response_with_provider_thinking("openai")],
        None,
        &[],
    )
    .unwrap();

    let content = request["messages"][0]["content"].as_array().unwrap();
    assert_eq!(content[0]["type"], "text");
    assert_eq!(content[0]["text"], "<think>\ninspect context\n</think>");
    assert!(content[0].get("signature").is_none());
}

#[test]
fn build_request_does_not_replay_ambiguous_legacy_thinking_signature() {
    let response = ModelMessage::Response(ModelResponse {
        parts: vec![ModelResponsePart::Thinking {
            text: "legacy inspect".to_string(),
            signature: Some("ambiguous-signature".to_string()),
        }],
        usage: starweaver_usage::Usage::default(),
        model_name: None,
        provider: None,
        finish_reason: None,
        timestamp: None,
        run_id: None,
        conversation_id: None,
        metadata: serde_json::Map::new(),
    });

    let request =
        AnthropicMessagesAdapter::build_request("claude-test", &[response], None, &[]).unwrap();

    let content = request["messages"][0]["content"].as_array().unwrap();
    assert_eq!(content[0]["type"], "text");
    assert_eq!(content[0]["text"], "<think>\nlegacy inspect\n</think>");
    assert!(content[0].get("signature").is_none());
    assert!(!serde_json::to_string(&request)
        .unwrap()
        .contains("ambiguous-signature"));
}

#[test]
fn parse_response_preserves_anthropic_provider_thinking() {
    let response = AnthropicMessagesAdapter::parse_response(&json!({
        "id": "msg_1",
        "model": "claude-test",
        "stop_reason": "end_turn",
        "content": [{
            "type": "thinking",
            "id": "thinking_1",
            "thinking": "inspect",
            "signature": "anthropic-signature"
        }],
        "usage": {"input_tokens": 1, "output_tokens": 2}
    }))
    .unwrap();

    assert!(matches!(
        &response.parts[0],
        ModelResponsePart::ProviderThinking { text, signature, provider }
            if text == "inspect"
                && signature.as_deref() == Some("anthropic-signature")
                && provider.provider_name.as_deref() == Some("anthropic")
                && provider.id.as_deref() == Some("thinking_1")
    ));
}
