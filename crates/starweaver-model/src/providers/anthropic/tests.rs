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
fn build_request_does_not_replay_ambiguous_thinking_signature() {
    let response = ModelMessage::Response(ModelResponse {
        parts: vec![ModelResponsePart::Thinking {
            text: "inspect context".to_string(),
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
    assert_eq!(content[0]["text"], "<think>\ninspect context\n</think>");
    assert!(content[0].get("signature").is_none());
    assert!(
        !serde_json::to_string(&request)
            .unwrap()
            .contains("ambiguous-signature")
    );
}

#[test]
fn build_request_replays_anthropic_redacted_thinking_natively() {
    let response = ModelMessage::Response(ModelResponse {
        parts: vec![ModelResponsePart::ProviderOpaque {
            item_type: "redacted_thinking".to_string(),
            payload: json!({
                "type": "redacted_thinking",
                "data": "encrypted-redacted-thinking"
            }),
            provider: ProviderPartInfo::new("anthropic"),
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
    assert_eq!(content[0]["type"], "redacted_thinking");
    assert_eq!(content[0]["data"], "encrypted-redacted-thinking");
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

#[test]
fn parse_response_preserves_anthropic_redacted_thinking() {
    let response = AnthropicMessagesAdapter::parse_response(&json!({
        "id": "msg_1",
        "model": "claude-test",
        "stop_reason": "end_turn",
        "content": [{
            "type": "redacted_thinking",
            "data": "encrypted-redacted-thinking"
        }],
        "usage": {"input_tokens": 1, "output_tokens": 2}
    }))
    .unwrap();

    assert!(matches!(
        &response.parts[0],
        ModelResponsePart::ProviderOpaque { item_type, payload, provider }
            if item_type == "redacted_thinking"
                && payload["data"] == "encrypted-redacted-thinking"
                && provider.provider_name.as_deref() == Some("anthropic")
    ));
}

#[test]
fn parse_response_preserves_reported_anthropic_cache_write_aggregate() {
    let response = AnthropicMessagesAdapter::parse_response(&json!({
        "id": "msg_cache",
        "model": "claude-test",
        "stop_reason": "end_turn",
        "content": [{"type": "text", "text": "done"}],
        "usage": {
            "input_tokens": 7,
            "output_tokens": 8,
            "cache_creation_input_tokens": 8,
            "cache_creation": {
                "ephemeral_5m_input_tokens": 4,
                "ephemeral_1h_input_tokens": 5
            },
            "cache_read_input_tokens": 10
        }
    }))
    .unwrap();

    assert_eq!(response.usage.input_tokens, 25);
    assert_eq!(response.usage.cache_write_tokens, 8);
    assert_eq!(response.usage.cache_write_1h_tokens, 5);
    assert_eq!(response.usage.cache_read_tokens, 10);
    assert_eq!(response.usage.total_tokens, 33);
    assert_eq!(response.usage.effective_cache_write_tokens(), 8);
    assert_eq!(response.usage.effective_cache_write_1h_tokens(), 5);
}
