use serde_json::Value;
use starweaver_core::ConversationId;

use crate::{
    ModelError,
    message::{ModelMessage, ModelResponse, ModelResponsePart},
};

use super::options::OpenAiReplayOptions;

type ServerSideStateMessages<'a> = (Option<String>, Option<String>, Vec<&'a ModelMessage>);

pub(super) fn resolve_server_side_state<'a>(
    messages: &'a [ModelMessage],
    replay: &OpenAiReplayOptions,
) -> Result<ServerSideStateMessages<'a>, ModelError> {
    if replay.previous_response_id.is_some() && replay.conversation_id.is_some() {
        return Err(ModelError::MessageMapping(
            "OpenAI Responses previous_response_id and conversation cannot both be set".to_string(),
        ));
    }
    if let Some(setting) = replay.conversation_id.as_deref() {
        let (conversation_id, trimmed) = resolve_conversation_id(messages, setting);
        return Ok((None, conversation_id, trimmed));
    }
    if let Some(setting) = replay.previous_response_id.as_deref() {
        let (previous_response_id, trimmed) = resolve_previous_response_id(messages, setting);
        return Ok((previous_response_id, None, trimmed));
    }
    Ok((None, None, messages.iter().collect()))
}

fn resolve_previous_response_id<'a>(
    messages: &'a [ModelMessage],
    setting: &str,
) -> (Option<String>, Vec<&'a ModelMessage>) {
    let mut trimmed = Vec::new();
    for message in messages.iter().rev() {
        if let ModelMessage::Response(response) = message
            && is_openai_response(response)
        {
            if is_compaction_boundary(response) {
                return (None, messages.iter().collect());
            }
            if let Some(response_id) = response
                .provider
                .as_ref()
                .and_then(|p| p.response_id.clone())
                && !trimmed.is_empty()
            {
                trimmed.reverse();
                return (Some(response_id), trimmed);
            }
            break;
        }
        trimmed.push(message);
    }
    if setting == "auto" || is_at_compaction_boundary(messages) {
        (None, messages.iter().collect())
    } else {
        (Some(setting.to_string()), messages.iter().collect())
    }
}

fn resolve_conversation_id<'a>(
    messages: &'a [ModelMessage],
    setting: &str,
) -> (Option<String>, Vec<&'a ModelMessage>) {
    if setting == "auto" {
        let active_conversation_id = messages.last().and_then(message_conversation_id);
        return get_conversation_id_and_new_messages(messages, None, active_conversation_id);
    }

    let (conversation_id, trimmed) =
        get_conversation_id_and_new_messages(messages, Some(setting), None);
    if conversation_id.is_some() {
        (conversation_id, trimmed)
    } else {
        (Some(setting.to_string()), messages.iter().collect())
    }
}

fn get_conversation_id_and_new_messages<'a>(
    messages: &'a [ModelMessage],
    expected_conversation_id: Option<&str>,
    active_conversation_id: Option<&str>,
) -> (Option<String>, Vec<&'a ModelMessage>) {
    let mut trimmed = Vec::new();
    for message in messages.iter().rev() {
        if let ModelMessage::Response(response) = message
            && is_openai_response(response)
        {
            if active_conversation_id.is_some()
                && response.conversation_id.is_some()
                && response
                    .conversation_id
                    .as_ref()
                    .map(ConversationId::as_str)
                    != active_conversation_id
            {
                trimmed.push(message);
                continue;
            }
            if let Some(conversation_id) = response
                .provider
                .as_ref()
                .and_then(|provider| provider.details.get("conversation_id"))
                .and_then(Value::as_str)
                .filter(|candidate| {
                    expected_conversation_id.is_none_or(|expected| expected == *candidate)
                })
            {
                trimmed.reverse();
                return (Some(conversation_id.to_string()), trimmed);
            }
        }
        trimmed.push(message);
    }
    (None, messages.iter().collect())
}

fn message_conversation_id(message: &ModelMessage) -> Option<&str> {
    match message {
        ModelMessage::Request(request) => {
            request.conversation_id.as_ref().map(ConversationId::as_str)
        }
        ModelMessage::Response(response) => response
            .conversation_id
            .as_ref()
            .map(ConversationId::as_str),
    }
}

fn is_openai_response(response: &ModelResponse) -> bool {
    response
        .provider
        .as_ref()
        .is_some_and(|provider| provider.name == "openai")
}

fn is_at_compaction_boundary(messages: &[ModelMessage]) -> bool {
    messages
        .iter()
        .rev()
        .find_map(|message| match message {
            ModelMessage::Response(response) if is_openai_response(response) => {
                Some(is_compaction_boundary(response))
            }
            ModelMessage::Request(_) | ModelMessage::Response(_) => None,
        })
        .unwrap_or(false)
}

fn is_compaction_boundary(response: &ModelResponse) -> bool {
    response.provider.as_ref().is_some_and(|provider| {
        provider
            .details
            .get("compaction")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }) || response.parts.iter().any(ModelResponsePart::is_compaction)
}
