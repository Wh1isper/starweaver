//! Anthropic response parsing.

use serde_json::Value;

use crate::{
    message::{
        FinishReason, ModelResponse, ModelResponsePart, ProviderInfo, ProviderPartInfo,
        ToolCallPart,
    },
    providers::usage_from_named_including_cache_input,
    ModelError,
};

#[allow(clippy::unnecessary_wraps)]
pub(super) fn parse_response(value: &Value) -> Result<ModelResponse, ModelError> {
    let mut parts = Vec::new();
    for block in value
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    parts.push(ModelResponsePart::Text {
                        text: text.to_string(),
                    });
                }
            }
            Some("thinking") => parts.push(ModelResponsePart::ProviderThinking {
                text: block
                    .get("thinking")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                signature: block
                    .get("signature")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                provider: block.get("id").and_then(Value::as_str).map_or_else(
                    || ProviderPartInfo::new("anthropic"),
                    |id| ProviderPartInfo::new("anthropic").with_id(id),
                ),
            }),
            Some("tool_use") => parts.push(ModelResponsePart::ToolCall(ToolCallPart {
                id: block
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                name: block
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                arguments: block.get("input").cloned().unwrap_or(Value::Null).into(),
            })),
            _ => {}
        }
    }

    Ok(ModelResponse {
        parts,
        usage: usage_from_named_including_cache_input(value, "input_tokens", "output_tokens"),
        model_name: value
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_string),
        provider: Some(ProviderInfo {
            name: "anthropic".to_string(),
            response_id: value.get("id").and_then(Value::as_str).map(str::to_string),
            details: serde_json::Map::new(),
        }),
        finish_reason: match value.get("stop_reason").and_then(Value::as_str) {
            Some("end_turn") => Some(FinishReason::Stop),
            Some("max_tokens") => Some(FinishReason::Length),
            Some("tool_use") => Some(FinishReason::ToolCalls),
            Some(_) => Some(FinishReason::Unknown),
            None => None,
        },
        timestamp: None,
        run_id: None,
        conversation_id: None,
        metadata: serde_json::Map::new(),
    })
}
