//! Error-aware retry recovery for model request retries.
//!
//! These helpers mirror the durable recovery behavior used by ya-agent-sdk's
//! `stream_agent(..., resume_on_error=True)`: before a higher-level retry sends
//! repaired history back to a provider, we remove provider-stale references and
//! aggressively reduce payloads that commonly trigger context overflow errors.

use serde_json::{Map, Value};
use starweaver_model::{
    ContentPart, ModelError, ModelMessage, ModelRequest, ModelRequestPart, ModelResponse,
    ModelResponsePart, ToolReturnPart,
};

const TOOL_RETURN_MAX_CHARS: usize = 500;
const TOOL_RETURN_KEEP_HEAD: usize = 200;
const TOOL_RETURN_KEEP_TAIL: usize = 200;

const MEDIA_REMOVED_REMINDER: &str = "<system-reminder>Media content was removed during retry recovery because the previous request exceeded the model context limit. If the media is still needed, ask the user to attach it again or inspect it with a focused tool call.</system-reminder>";
const RESPONSE_MEDIA_REMOVED_TEXT: &str = "<system-reminder>Assistant media content was removed during retry recovery because the previous request exceeded the model context limit.</system-reminder>";

const CONTEXT_OVERFLOW_PATTERNS: &[&str] = &[
    "context_length_exceeded",
    "maximum context length",
    "max context length",
    "context window",
    "context limit",
    "context too long",
    "prompt is too long",
    "prompt too long",
    "too many tokens",
    "token count exceeds maximum",
    "exceeds maximum token",
    "exceed the maximum number of tokens",
    "input is too long",
    "input too long",
    "reduce the length of the messages",
    "reduce the size of your message",
    "messages resulted in",
    "requested tokens",
];

const OPENAI_REFERENCE_PATTERNS: &[&str] = &[
    "invalid_encrypted_content",
    "encrypted_content",
    "item_not_found",
    "item not found",
    "no item with id",
    "could not find item",
    "was provided without its required following item",
    "required following item",
    "previous_response_id",
    "previous response",
];

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

/// Trim older large tool returns and remove image/video media in place.
pub fn heal_context_overflow_history(history: &mut [ModelMessage]) -> bool {
    let trimmed = trim_tool_returns(history) > 0;
    let stripped = strip_image_video_media(history);
    trimmed || stripped
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
    if let Some(provider) = &mut response.provider {
        if provider.name == "openai" {
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

fn trim_tool_returns(history: &mut [ModelMessage]) -> usize {
    let mut trimmed = 0usize;

    for message in history {
        let ModelMessage::Request(request) = message else {
            continue;
        };
        for part in &mut request.parts {
            let ModelRequestPart::ToolReturn(tool_return) = part else {
                continue;
            };
            if truncate_tool_return_content(tool_return) {
                trimmed = trimmed.saturating_add(1);
            }
        }
    }

    trimmed
}

fn truncate_tool_return_content(tool_return: &mut ToolReturnPart) -> bool {
    let content = tool_content_text(&tool_return.content);
    if content.chars().count() <= TOOL_RETURN_MAX_CHARS {
        return false;
    }
    let original_chars = content.chars().count();
    tool_return.content = Value::String(truncate_tool_content(&content));
    tool_return.metadata.insert(
        "starweaver_retry_recovery_truncated".to_string(),
        Value::Bool(true),
    );
    tool_return.metadata.insert(
        "starweaver_retry_recovery_original_chars".to_string(),
        serde_json::json!(original_chars),
    );

    if let Some(user_content) = &mut tool_return.user_content {
        let user_text = tool_content_text(user_content);
        if user_text.chars().count() > TOOL_RETURN_MAX_CHARS {
            *user_content = Value::String(truncate_tool_content(&user_text));
        }
    }

    true
}

fn tool_content_text(value: &Value) -> String {
    value
        .as_str()
        .map_or_else(|| value.to_string(), str::to_string)
}

fn truncate_tool_content(content: &str) -> String {
    let total = content.chars().count();
    if total <= TOOL_RETURN_MAX_CHARS {
        return content.to_string();
    }
    let head = content
        .chars()
        .take(TOOL_RETURN_KEEP_HEAD)
        .collect::<String>();
    let tail = content
        .chars()
        .rev()
        .take(TOOL_RETURN_KEEP_TAIL)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    let truncated = total.saturating_sub(TOOL_RETURN_KEEP_HEAD + TOOL_RETURN_KEEP_TAIL);
    format!("{head}\n[... {truncated} chars truncated ...]\n{tail}")
}

fn strip_image_video_media(history: &mut [ModelMessage]) -> bool {
    let mut changed = false;
    for message in history {
        match message {
            ModelMessage::Request(request) => {
                for part in &mut request.parts {
                    match part {
                        ModelRequestPart::UserPrompt { content, .. } => {
                            for item in content {
                                changed |= replace_media_content_part(item);
                            }
                        }
                        ModelRequestPart::ToolReturn(tool_return) => {
                            changed |= replace_media_value(&mut tool_return.content);
                        }
                        ModelRequestPart::SystemPrompt { .. }
                        | ModelRequestPart::RetryPrompt { .. }
                        | ModelRequestPart::Instruction { .. } => {}
                    }
                }
            }
            ModelMessage::Response(response) => {
                for part in &mut response.parts {
                    if matches!(part, ModelResponsePart::File { media_type, .. } if is_image_or_video_media_type(media_type))
                    {
                        *part = ModelResponsePart::Text {
                            text: RESPONSE_MEDIA_REMOVED_TEXT.to_string(),
                        };
                        changed = true;
                    }
                }
            }
        }
    }
    changed
}

fn replace_media_content_part(part: &mut ContentPart) -> bool {
    if is_image_or_video_content_part(part) {
        *part = ContentPart::Text {
            text: MEDIA_REMOVED_REMINDER.to_string(),
        };
        return true;
    }
    false
}

fn is_image_or_video_content_part(part: &ContentPart) -> bool {
    match part {
        ContentPart::ImageUrl { .. } => true,
        ContentPart::FileUrl { media_type, .. }
        | ContentPart::Binary { media_type, .. }
        | ContentPart::ResourceRef { media_type, .. }
        | ContentPart::DataUrl { media_type, .. } => is_image_or_video_media_type(media_type),
        ContentPart::Text { .. } => false,
    }
}

fn replace_media_value(value: &mut Value) -> bool {
    match value {
        Value::Array(items) => {
            let mut changed = false;
            for item in items {
                changed |= replace_media_value(item);
            }
            changed
        }
        Value::Object(object) => {
            if value_object_looks_like_image_or_video(object) {
                *value = Value::String(MEDIA_REMOVED_REMINDER.to_string());
                true
            } else {
                let mut changed = false;
                for item in object.values_mut() {
                    changed |= replace_media_value(item);
                }
                changed
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => false,
    }
}

fn value_object_looks_like_image_or_video(object: &Map<String, Value>) -> bool {
    let kind = object.get("kind").and_then(Value::as_str);
    if matches!(kind, Some("image_url" | "image" | "video")) {
        return true;
    }
    object
        .get("media_type")
        .and_then(Value::as_str)
        .is_some_and(is_image_or_video_media_type)
        && (object.contains_key("data")
            || object.contains_key("data_url")
            || object.contains_key("url")
            || object.contains_key("uri"))
}

fn is_image_or_video_media_type(media_type: &str) -> bool {
    media_type.starts_with("image/") || media_type.starts_with("video/")
}

fn is_openai_item_reference_error(error_text: &str) -> bool {
    let lowered = error_text.to_ascii_lowercase();
    OPENAI_REFERENCE_PATTERNS
        .iter()
        .any(|pattern| lowered.contains(pattern))
        || (lowered.contains("item")
            && (lowered.contains("not found") || lowered.contains("required following item")))
}

fn is_context_overflow_error(error_text: &str) -> bool {
    let lowered = error_text.to_ascii_lowercase();
    if !CONTEXT_OVERFLOW_PATTERNS
        .iter()
        .any(|pattern| lowered.contains(pattern))
    {
        return false;
    }
    lowered.contains("token")
        || lowered.contains("context")
        || lowered.contains("prompt")
        || lowered.contains("message")
        || lowered.contains("input")
}

fn model_error_text(error: &ModelError) -> String {
    let mut parts = Vec::new();
    collect_model_error_text(error, &mut parts);
    parts.join("\n")
}

fn collect_model_error_text(error: &ModelError, parts: &mut Vec<String>) {
    parts.push(format!("{error:?}"));
    parts.push(error.to_string());
    match error {
        ModelError::ProviderStatus { body, .. } => parts.push(body.to_string()),
        ModelError::RetryExhausted { source, .. } => collect_model_error_text(source, parts),
        ModelError::MessageMapping(_)
        | ModelError::ResponseParsing(_)
        | ModelError::Transport(_)
        | ModelError::RealModelRequestBlocked { .. }
        | ModelError::UnsupportedResponse(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use starweaver_model::{ProviderInfo, ProviderPartInfo, ToolCallPart};

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
            usage: starweaver_core::Usage::default(),
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
            usage: starweaver_core::Usage::default(),
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
                usage: starweaver_core::Usage::default(),
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
