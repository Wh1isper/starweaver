//! Error-aware retry recovery for model request retries.
//!
//! These helpers implement durable model-error recovery: before a higher-level retry sends
//! repaired history back to a provider, we remove provider-stale references and
//! aggressively reduce payloads that commonly trigger context overflow errors.

use starweaver_model::{ModelError, ModelMessage};

mod classify;
mod openai;
mod overflow;

pub use openai::heal_openai_item_reference_history;
pub use overflow::heal_context_overflow_history;

use classify::{is_context_overflow_error, is_openai_item_reference_error, model_error_text};

/// Built-in resume prompt used after a recoverable model request failure.
pub const DEFAULT_MODEL_ERROR_RESUME_PROMPT: &str = "The previous streaming model request failed before the agent finished.\nContinue the task from the available conversation history. Avoid repeating completed work.";

/// Result of retry message recovery.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RetryRecoveryResult {
    /// Recovered message history.
    pub history: Vec<ModelMessage>,
    /// Whether any recovery step changed the history.
    pub changed: bool,
    /// Recovery reasons, such as `openai_item_reference` or `context_overflow`.
    pub reasons: Vec<&'static str>,
}

/// Apply built-in recovery policies to model history based on an upstream model error.
#[must_use]
pub fn recover_retry_message_history(
    error: &ModelError,
    history: &[ModelMessage],
) -> RetryRecoveryResult {
    let mut recovered = history.to_vec();
    if recovered.is_empty() {
        return RetryRecoveryResult::default();
    }

    let error_text = model_error_text(error);
    let mut reasons = Vec::new();
    let mut changed = false;

    if is_openai_item_reference_error(&error_text) {
        let item_changed = heal_openai_item_reference_history(&mut recovered);
        if item_changed {
            changed = true;
            reasons.push("openai_item_reference");
        }
    }

    if is_context_overflow_error(&error_text) {
        let overflow_changed = heal_context_overflow_history(&mut recovered);
        if overflow_changed {
            changed = true;
            reasons.push("context_overflow");
        }
    }

    RetryRecoveryResult {
        history: recovered,
        changed,
        reasons,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Map, Value};
    use starweaver_model::{
        ContentPart, ModelRequest, ModelRequestPart, ModelResponse, ModelResponsePart,
        ProviderInfo, ProviderPartInfo, ToolCallPart, ToolReturnPart,
    };

    #[test]
    fn context_overflow_recovery_trims_old_tool_returns_and_strips_media() {
        let mut history = vec![
            ModelMessage::Request(ModelRequest {
                parts: vec![ModelRequestPart::ToolReturn(ToolReturnPart::new(
                    "call_1",
                    "view",
                    Value::String("A".repeat(2_000)),
                ))],
                timestamp: None,
                instructions: None,
                run_id: None,
                conversation_id: None,
                metadata: Map::new(),
            }),
            ModelMessage::Response(ModelResponse::text("processed")),
            ModelMessage::Request(ModelRequest {
                parts: vec![ModelRequestPart::UserPrompt {
                    content: vec![
                        ContentPart::Text {
                            text: "inspect".to_string(),
                        },
                        ContentPart::Binary {
                            data: b"image".to_vec(),
                            media_type: "image/png".to_string(),
                        },
                    ],
                    name: None,
                    metadata: Map::new(),
                }],
                timestamp: None,
                instructions: None,
                run_id: None,
                conversation_id: None,
                metadata: Map::new(),
            }),
        ];

        let changed = heal_context_overflow_history(&mut history);

        assert!(changed);
        let ModelMessage::Request(request) = &history[0] else {
            panic!("request")
        };
        let ModelRequestPart::ToolReturn(tool_return) = &request.parts[0] else {
            panic!("tool return")
        };
        assert!(tool_return
            .content
            .as_str()
            .is_some_and(|content| content.contains("truncated")));
        let ModelMessage::Request(media_request) = &history[2] else {
            panic!("request")
        };
        let ModelRequestPart::UserPrompt { content, .. } = &media_request.parts[0] else {
            panic!("user prompt")
        };
        assert!(
            matches!(&content[1], ContentPart::Text { text } if text.contains("Media content was removed"))
        );
    }

    #[test]
    fn context_overflow_recovery_trims_latest_tool_return_after_response() {
        let mut history = vec![
            ModelMessage::Response(ModelResponse::text("tool requested")),
            ModelMessage::Request(ModelRequest {
                parts: vec![ModelRequestPart::ToolReturn(ToolReturnPart::new(
                    "call_latest",
                    "view",
                    Value::String("B".repeat(2_000)),
                ))],
                timestamp: None,
                instructions: None,
                run_id: None,
                conversation_id: None,
                metadata: Map::new(),
            }),
        ];

        let changed = heal_context_overflow_history(&mut history);

        assert!(changed);
        let ModelMessage::Request(request) = &history[1] else {
            panic!("request")
        };
        let ModelRequestPart::ToolReturn(tool_return) = &request.parts[0] else {
            panic!("tool return")
        };
        assert!(tool_return
            .content
            .as_str()
            .is_some_and(|content| content.contains("truncated")));
        assert_eq!(
            tool_return
                .metadata
                .get("starweaver_retry_recovery_truncated"),
            Some(&Value::Bool(true))
        );
    }

    #[test]
    fn openai_reference_recovery_preserves_non_openai_provider_metadata() {
        let mut details = Map::new();
        details.insert(
            "encrypted_content".to_string(),
            Value::String("anthropic-signature".to_string()),
        );
        let mut history = vec![ModelMessage::Response(ModelResponse {
            parts: vec![
                ModelResponsePart::ProviderThinking {
                    text: "inspect".to_string(),
                    signature: Some("anthropic-signature".to_string()),
                    provider: ProviderPartInfo::new("anthropic")
                        .with_id("thinking_1")
                        .with_details(details),
                },
                ModelResponsePart::ProviderOpaque {
                    item_type: "anthropic_native".to_string(),
                    payload: serde_json::json!({
                        "type": "anthropic_native",
                        "id": "anthropic_item_1",
                        "signature": "anthropic-signature"
                    }),
                    provider: ProviderPartInfo::new("anthropic").with_id("anthropic_item_1"),
                },
            ],
            usage: starweaver_usage::Usage::default(),
            model_name: None,
            provider: Some(ProviderInfo {
                name: "anthropic".to_string(),
                response_id: Some("msg_1".to_string()),
                details: Map::new(),
            }),
            finish_reason: None,
            timestamp: None,
            run_id: None,
            conversation_id: None,
            metadata: Map::new(),
        })];

        let changed = heal_openai_item_reference_history(&mut history);

        assert!(!changed);
        let ModelMessage::Response(response) = &history[0] else {
            panic!("response")
        };
        assert_eq!(
            response
                .provider
                .as_ref()
                .and_then(|provider| provider.response_id.as_deref()),
            Some("msg_1")
        );
        assert!(matches!(
            &response.parts[0],
            ModelResponsePart::ProviderThinking { signature, provider, .. }
                if signature.as_deref() == Some("anthropic-signature")
                    && provider.id.as_deref() == Some("thinking_1")
                    && provider.details.get("encrypted_content").and_then(Value::as_str) == Some("anthropic-signature")
        ));
        assert!(matches!(
            &response.parts[1],
            ModelResponsePart::ProviderOpaque { payload, provider, .. }
                if provider.id.as_deref() == Some("anthropic_item_1")
                    && payload.get("id").and_then(Value::as_str) == Some("anthropic_item_1")
        ));
    }

    #[test]
    fn openai_reference_recovery_scrubs_provider_opaque_payload_references() {
        let mut history = vec![ModelMessage::Response(ModelResponse {
            parts: vec![ModelResponsePart::ProviderOpaque {
                item_type: "mcp_call".to_string(),
                payload: serde_json::json!({
                    "type": "mcp_call",
                    "id": "mcp_1",
                    "call_id": "call_1",
                    "encrypted_content": "encrypted",
                    "namespace": "tools",
                    "nested": {
                        "response_id": "resp_1",
                        "conversation_id": "conv_1",
                        "items": [{"id": "nested_1", "call_id": "nested_call"}]
                    }
                }),
                provider: ProviderPartInfo::new("openai")
                    .with_id("mcp_1")
                    .with_details({
                        let mut details = Map::new();
                        details.insert(
                            "encrypted_content".to_string(),
                            Value::String("encrypted".to_string()),
                        );
                        details.insert("raw_content".to_string(), serde_json::json!(["raw"]));
                        details
                    }),
            }],
            usage: starweaver_usage::Usage::default(),
            model_name: None,
            provider: None,
            finish_reason: None,
            timestamp: None,
            run_id: None,
            conversation_id: None,
            metadata: Map::new(),
        })];

        let changed = heal_openai_item_reference_history(&mut history);

        assert!(changed);
        let ModelMessage::Response(response) = &history[0] else {
            panic!("response")
        };
        let ModelResponsePart::ProviderOpaque {
            payload, provider, ..
        } = &response.parts[0]
        else {
            panic!("provider opaque")
        };
        assert!(provider.id.is_none());
        assert!(provider.details.get("encrypted_content").is_none());
        assert!(provider.details.get("raw_content").is_none());
        let serialized = payload.to_string();
        assert!(!serialized.contains("mcp_1"));
        assert!(!serialized.contains("call_1"));
        assert!(!serialized.contains("encrypted"));
        assert!(!serialized.contains("resp_1"));
        assert!(!serialized.contains("conv_1"));
        assert!(!serialized.contains("nested_1"));
        assert_eq!(payload["type"], "mcp_call");
        let Some(items) = payload["nested"]["items"].as_array() else {
            panic!("nested items should be an array");
        };
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn openai_reference_recovery_drops_response_ids_and_rewrites_tool_ids() {
        let mut history = vec![
            ModelMessage::Response(ModelResponse {
                parts: vec![ModelResponsePart::ToolCall(ToolCallPart {
                    id: "call_1|fc_1".to_string(),
                    name: "lookup".to_string(),
                    arguments: Value::Null.into(),
                })],
                usage: starweaver_usage::Usage::default(),
                model_name: None,
                provider: Some(ProviderInfo {
                    name: "openai".to_string(),
                    response_id: Some("resp_1".to_string()),
                    details: Map::new(),
                }),
                finish_reason: None,
                timestamp: None,
                run_id: None,
                conversation_id: None,
                metadata: Map::new(),
            }),
            ModelMessage::Request(ModelRequest {
                parts: vec![ModelRequestPart::ToolReturn(ToolReturnPart::new(
                    "call_1|fc_1",
                    "lookup",
                    Value::String("ok".to_string()),
                ))],
                timestamp: None,
                instructions: None,
                run_id: None,
                conversation_id: None,
                metadata: Map::new(),
            }),
        ];

        let changed = heal_openai_item_reference_history(&mut history);

        assert!(changed);
        let ModelMessage::Response(response) = &history[0] else {
            panic!("response")
        };
        assert_eq!(
            response
                .provider
                .as_ref()
                .and_then(|provider| provider.response_id.as_ref()),
            None
        );
        assert!(
            matches!(&response.parts[0], ModelResponsePart::ToolCall(call) if call.id == "call_1")
        );
        let ModelMessage::Request(request) = &history[1] else {
            panic!("request")
        };
        assert!(
            matches!(&request.parts[0], ModelRequestPart::ToolReturn(tool_return) if tool_return.tool_call_id == "call_1")
        );
    }
}
