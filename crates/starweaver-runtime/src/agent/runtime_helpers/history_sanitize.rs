//! Message history sanitization helpers.

use std::collections::BTreeSet;

use starweaver_model::{ModelMessage, ModelRequestPart, ModelResponsePart};

pub(super) fn sanitize_incomplete_tool_call_history(
    messages: Vec<ModelMessage>,
) -> Vec<ModelMessage> {
    if messages.is_empty() {
        return messages;
    }

    let valid_tool_call_ids = valid_tool_call_ids(&messages);
    let mut pending_tool_call_ids = BTreeSet::new();
    let mut sanitized = Vec::with_capacity(messages.len());

    for message in messages {
        match message {
            ModelMessage::Response(mut response) => {
                response.parts.retain(|part| match part {
                    ModelResponsePart::ToolCall(call)
                    | ModelResponsePart::ProviderToolCall { call, .. } => {
                        valid_tool_call_ids.contains(&call.id)
                    }
                    ModelResponsePart::Text { text }
                    | ModelResponsePart::ProviderText { text, .. }
                    | ModelResponsePart::Thinking { text, .. }
                    | ModelResponsePart::ProviderThinking { text, .. } => !text.is_empty(),
                    ModelResponsePart::Compaction { summary } => !summary.is_empty(),
                    ModelResponsePart::NativeToolCall { .. }
                    | ModelResponsePart::NativeToolReturn { .. }
                    | ModelResponsePart::File { .. }
                    | ModelResponsePart::ProviderOpaque { .. } => true,
                });
                for call in response.tool_calls() {
                    pending_tool_call_ids.insert(call.id);
                }
                if !response.parts.is_empty() {
                    sanitized.push(ModelMessage::Response(response));
                }
            }
            ModelMessage::Request(mut request) => {
                request.parts.retain(|part| match part {
                    ModelRequestPart::ToolReturn(tool_return) => {
                        pending_tool_call_ids.remove(&tool_return.tool_call_id)
                            || tool_return
                                .metadata
                                .get("starweaver_retry_recovery_truncated")
                                .and_then(serde_json::Value::as_bool)
                                .unwrap_or(false)
                    }
                    ModelRequestPart::RetryPrompt {
                        tool_call_id: Some(tool_call_id),
                        ..
                    } => valid_tool_call_ids.contains(tool_call_id),
                    ModelRequestPart::SystemPrompt { .. }
                    | ModelRequestPart::UserPrompt { .. }
                    | ModelRequestPart::RetryPrompt {
                        tool_call_id: None, ..
                    }
                    | ModelRequestPart::Instruction { .. } => true,
                });
                if !request.parts.is_empty() {
                    sanitized.push(ModelMessage::Request(request));
                }
            }
        }
    }

    sanitized
}

fn valid_tool_call_ids(messages: &[ModelMessage]) -> BTreeSet<String> {
    let mut valid = BTreeSet::new();

    for (message_index, message) in messages.iter().enumerate() {
        let ModelMessage::Response(response) = message else {
            continue;
        };
        for call in response.tool_calls() {
            if has_following_tool_return_before_barrier(messages, message_index, &call.id) {
                valid.insert(call.id);
            }
        }
    }

    valid
}

fn has_following_tool_return_before_barrier(
    messages: &[ModelMessage],
    response_index: usize,
    tool_call_id: &str,
) -> bool {
    for message in messages.iter().skip(response_index.saturating_add(1)) {
        match message {
            ModelMessage::Response(_) => return false,
            ModelMessage::Request(request) => {
                let mut has_barrier = false;
                for part in &request.parts {
                    match part {
                        ModelRequestPart::ToolReturn(tool_return) => {
                            if tool_return.tool_call_id == tool_call_id {
                                return true;
                            }
                        }
                        ModelRequestPart::SystemPrompt { .. }
                        | ModelRequestPart::UserPrompt { .. }
                        | ModelRequestPart::RetryPrompt { .. }
                        | ModelRequestPart::Instruction { .. } => has_barrier = true,
                    }
                }
                if has_barrier {
                    return false;
                }
            }
        }
    }

    false
}
