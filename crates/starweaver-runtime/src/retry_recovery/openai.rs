//! `OpenAI` Responses replay-reference healing.

use serde_json::{Map, Value};
use starweaver_model::{
    ModelMessage, ModelRequest, ModelRequestPart, ModelResponse, ModelResponsePart,
};

/// Remove provider-stale `OpenAI` Responses references from history in place.
pub fn heal_openai_item_reference_history(history: &mut [ModelMessage]) -> bool {
    let mut changed = false;
    let mut tool_call_id_map = Map::new();

    for message in history {
        match message {
            ModelMessage::Response(response) => {
                changed |= heal_openai_response(response, &mut tool_call_id_map);
            }
            ModelMessage::Request(request) => {
                changed |= heal_request_tool_call_ids(request, &tool_call_id_map);
            }
        }
    }

    changed
}

fn heal_openai_response(
    response: &mut ModelResponse,
    tool_call_id_map: &mut Map<String, Value>,
) -> bool {
    let mut changed = false;
    let response_is_openai = response
        .provider
        .as_ref()
        .is_some_and(|provider| provider.name == "openai");
    if let Some(provider) = &mut response.provider
        && provider.name == "openai"
    {
        if provider.response_id.take().is_some() {
            changed = true;
        }
        changed |= drop_metadata_keys(
            &mut provider.details,
            &[
                "conversation_id",
                "encrypted_content",
                "previous_response_id",
                "response_id",
                "usage",
            ],
        );
    }
    if response_is_openai {
        changed |= drop_metadata_keys(
            &mut response.metadata,
            &[
                "conversation_id",
                "encrypted_content",
                "previous_response_id",
                "response_id",
            ],
        );
    }

    for part in &mut response.parts {
        match part {
            ModelResponsePart::Thinking { signature, .. } => {
                if response_is_openai && signature.take().is_some() {
                    changed = true;
                }
            }
            ModelResponsePart::ProviderThinking {
                signature,
                provider,
                ..
            } => {
                if provider_part_is_openai(provider, response_is_openai)
                    && signature.take().is_some()
                {
                    changed = true;
                }
                changed |= heal_provider_part_references(part, response_is_openai);
            }
            ModelResponsePart::ToolCall(call) => {
                if response_is_openai {
                    let original = call.id.clone();
                    let healed = strip_openai_compound_id(&original);
                    if healed != original {
                        call.id.clone_from(&healed);
                        tool_call_id_map.insert(original, Value::String(healed));
                        changed = true;
                    }
                }
            }
            ModelResponsePart::ProviderToolCall { call, provider } => {
                if provider_part_is_openai(provider, response_is_openai) {
                    let original = call.id.clone();
                    let healed = strip_openai_compound_id(&original);
                    if healed != original {
                        call.id.clone_from(&healed);
                        tool_call_id_map.insert(original, Value::String(healed));
                        changed = true;
                    }
                }
                changed |= heal_provider_part_references(part, response_is_openai);
            }
            ModelResponsePart::Text { .. }
            | ModelResponsePart::ProviderText { .. }
            | ModelResponsePart::NativeToolCall { .. }
            | ModelResponsePart::NativeToolReturn { .. }
            | ModelResponsePart::File { .. }
            | ModelResponsePart::Compaction { .. }
            | ModelResponsePart::ProviderOpaque { .. } => {
                changed |= heal_provider_part_references(part, response_is_openai);
            }
        }
    }

    changed
}

fn heal_request_tool_call_ids(
    request: &mut ModelRequest,
    tool_call_id_map: &Map<String, Value>,
) -> bool {
    let mut changed = false;
    for part in &mut request.parts {
        match part {
            ModelRequestPart::ToolReturn(tool_return) => {
                if let Some(healed) = tool_call_id_map
                    .get(&tool_return.tool_call_id)
                    .and_then(Value::as_str)
                {
                    tool_return.tool_call_id = healed.to_string();
                    changed = true;
                }
            }
            ModelRequestPart::RetryPrompt {
                tool_call_id: Some(tool_call_id),
                ..
            } => {
                if let Some(healed) = tool_call_id_map.get(tool_call_id).and_then(Value::as_str) {
                    *tool_call_id = healed.to_string();
                    changed = true;
                }
            }
            ModelRequestPart::SystemPrompt { .. }
            | ModelRequestPart::UserPrompt { .. }
            | ModelRequestPart::RetryPrompt {
                tool_call_id: None, ..
            }
            | ModelRequestPart::Instruction { .. } => {}
        }
    }
    changed
}

fn strip_openai_compound_id(value: &str) -> String {
    value
        .split_once('|')
        .map_or_else(|| value.to_string(), |(head, _)| head.to_string())
}

fn heal_provider_part_references(part: &mut ModelResponsePart, response_is_openai: bool) -> bool {
    match part {
        ModelResponsePart::ProviderText { provider, .. }
        | ModelResponsePart::ProviderThinking { provider, .. }
        | ModelResponsePart::ProviderToolCall { provider, .. }
            if provider_part_is_openai(provider, response_is_openai) =>
        {
            heal_provider_part_info(provider)
        }
        ModelResponsePart::ProviderOpaque {
            provider, payload, ..
        } if provider_part_is_openai(provider, response_is_openai) => {
            let provider_changed = heal_provider_part_info(provider);
            let payload_changed = scrub_provider_opaque_payload_references(payload);
            provider_changed || payload_changed
        }
        ModelResponsePart::Text { .. }
        | ModelResponsePart::ProviderText { .. }
        | ModelResponsePart::Thinking { .. }
        | ModelResponsePart::ProviderThinking { .. }
        | ModelResponsePart::ToolCall(_)
        | ModelResponsePart::ProviderToolCall { .. }
        | ModelResponsePart::NativeToolCall { .. }
        | ModelResponsePart::NativeToolReturn { .. }
        | ModelResponsePart::File { .. }
        | ModelResponsePart::Compaction { .. }
        | ModelResponsePart::ProviderOpaque { .. } => false,
    }
}

fn provider_part_is_openai(
    provider: &starweaver_model::ProviderPartInfo,
    response_is_openai: bool,
) -> bool {
    provider.is_provider("openai") || (response_is_openai && provider.provider_name.is_none())
}

fn heal_provider_part_info(provider: &mut starweaver_model::ProviderPartInfo) -> bool {
    let mut changed = provider.id.take().is_some();
    changed |= drop_metadata_keys(
        &mut provider.details,
        &[
            "encrypted_content",
            "raw_content",
            "previous_response_id",
            "response_id",
            "conversation_id",
            "namespace",
        ],
    );
    changed
}

fn scrub_provider_opaque_payload_references(payload: &mut Value) -> bool {
    const REPLAY_REFERENCE_KEYS: &[&str] = &[
        "id",
        "call_id",
        "encrypted_content",
        "previous_response_id",
        "response_id",
        "conversation_id",
        "namespace",
    ];

    match payload {
        Value::Object(object) => {
            let mut changed = drop_metadata_keys(object, REPLAY_REFERENCE_KEYS);
            for value in object.values_mut() {
                changed |= scrub_provider_opaque_payload_references(value);
            }
            changed
        }
        Value::Array(items) => {
            let mut changed = false;
            for value in items {
                changed |= scrub_provider_opaque_payload_references(value);
            }
            changed
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => false,
    }
}

fn drop_metadata_keys(metadata: &mut Map<String, Value>, keys: &[&str]) -> bool {
    let before = metadata.len();
    for key in keys {
        metadata.remove(*key);
    }
    metadata.len() != before
}
