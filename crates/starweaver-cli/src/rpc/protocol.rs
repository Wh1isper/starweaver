use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use starweaver_stream::{DisplayMessage, ReplayCursor, ReplayEvent, ReplayEventKind, ReplayScope};

use crate::{runtime_coordinator::RunAttachment, service::display_message_to_agui_event};

use super::RpcError;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum StreamPayloadFormat {
    #[default]
    Agui,
    DisplayMessage,
}

impl StreamPayloadFormat {
    pub(super) fn parse(value: Option<&str>) -> Result<Self, RpcError> {
        match value.unwrap_or("agui") {
            "agui" | "agui_json" | "agui-json" => Ok(Self::Agui),
            "display_message" | "display-message" | "display_json" | "display-json" => {
                Ok(Self::DisplayMessage)
            }
            other => Err(RpcError::new(
                -32_602,
                format!("unknown stream payload format: {other}"),
            )),
        }
    }

    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Agui => "agui",
            Self::DisplayMessage => "display_message",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RunOutputItem {
    session_id: String,
    run_id: String,
    cursor: ReplayCursor,
    payload_format: StreamPayloadFormat,
    payload: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_message: Option<DisplayMessage>,
}

pub(super) fn stream_payload_format(params: &Value) -> Result<StreamPayloadFormat, RpcError> {
    let value = params
        .get("stream")
        .and_then(|stream| {
            stream
                .get("payloadFormat")
                .or_else(|| stream.get("format"))
                .and_then(Value::as_str)
        })
        .or_else(|| params.get("payloadFormat").and_then(Value::as_str))
        .or_else(|| params.get("format").and_then(Value::as_str));
    StreamPayloadFormat::parse(value)
}

pub(super) fn replay_cursor_from_params(
    params: &Value,
    default_scope: ReplayScope,
) -> Result<Option<ReplayCursor>, RpcError> {
    if let Some(cursor) = params.get("cursor") {
        let cursor = serde_json::from_value::<ReplayCursor>(cursor.clone())
            .map_err(|error| RpcError::new(-32_602, format!("invalid cursor: {error}")))?;
        cursor
            .validate_scope(&default_scope)
            .map_err(|error| RpcError::new(-32_602, error.to_string()))?;
        return Ok(Some(cursor));
    }
    Ok(optional_usize(params, "after").map(|sequence| ReplayCursor::new(default_scope, sequence)))
}

pub(super) fn attachment_result(attachment: &RunAttachment, format: StreamPayloadFormat) -> Value {
    let events = attachment
        .events
        .iter()
        .filter_map(|event| output_item(event, format))
        .collect::<Vec<_>>();
    json!({
        "sessionId": attachment.session_id,
        "runId": attachment.run_id,
        "active": attachment.active,
        "payloadFormat": format.as_str(),
        "events": events,
    })
}

pub(super) fn replay_result(
    session_id: &str,
    run_id: Option<&str>,
    scope: &ReplayScope,
    events: &[ReplayEvent],
    requested_cursor: Option<&ReplayCursor>,
    next_sequence: usize,
) -> Value {
    let messages = display_messages(events);
    let latest_cursor = events
        .last()
        .map(|event| ReplayCursor::new(event.scope.clone(), event.sequence))
        .or_else(|| requested_cursor.cloned());
    json!({
        "sessionId": session_id,
        "runId": run_id,
        "scope": scope,
        "latestCursor": latest_cursor,
        "nextSequence": next_sequence,
        "events": events,
        "messages": messages,
    })
}

pub(super) fn output_item(
    event: &ReplayEvent,
    format: StreamPayloadFormat,
) -> Option<RunOutputItem> {
    let ReplayEventKind::DisplayMessage(message) = &event.event else {
        return None;
    };
    let display_message = (**message).clone();
    let payload = match format {
        StreamPayloadFormat::Agui => display_message_to_agui_event(&display_message)
            .unwrap_or_else(|| json!(display_message)),
        StreamPayloadFormat::DisplayMessage => json!(display_message),
    };
    Some(RunOutputItem {
        session_id: display_message.session_id.as_str().to_string(),
        run_id: display_message.run_id.as_str().to_string(),
        cursor: ReplayCursor::new(event.scope.clone(), event.sequence),
        payload_format: format,
        payload,
        display_message: matches!(format, StreamPayloadFormat::DisplayMessage)
            .then_some(display_message),
    })
}

fn display_messages(events: &[ReplayEvent]) -> Vec<DisplayMessage> {
    events
        .iter()
        .filter_map(|event| match &event.event {
            ReplayEventKind::DisplayMessage(message) => Some((**message).clone()),
            _ => None,
        })
        .collect()
}

pub(super) fn notification(method: &str, params: &Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
    })
}

fn optional_usize(params: &Value, key: &str) -> Option<usize> {
    params
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
}
