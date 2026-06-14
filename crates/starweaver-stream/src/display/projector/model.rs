//! Model response display projection.

use serde_json::json;
use starweaver_core::RunId;
use starweaver_model::{ModelResponse, ModelResponsePart, StreamDelta};
use starweaver_runtime::ModelResponseStreamEvent;

use super::super::{DisplayMessage, DisplayMessageKind, DisplayProjectionContext};
use super::tool::project_tool_call_messages;

pub(super) fn project_model_stream(
    context: &DisplayProjectionContext,
    sequence: usize,
    run_id: RunId,
    event: &ModelResponseStreamEvent,
) -> Vec<DisplayMessage> {
    match event {
        ModelResponseStreamEvent::PartStart(part) => project_stream_part_start(
            context,
            sequence,
            run_id,
            part.index,
            part.part_kind.as_str(),
        ),
        ModelResponseStreamEvent::PartDelta(delta) => {
            let Some((kind, payload)) = stream_delta_message_payload(delta.index, &delta.delta)
            else {
                return Vec::new();
            };
            let mut message =
                DisplayMessage::new(sequence, context.session_id.clone(), run_id, kind)
                    .with_payload(payload)
                    .with_preview(delta.as_text());
            if matches!(&delta.delta, StreamDelta::Thinking { .. }) {
                message
                    .metadata
                    .insert("reasoning".to_string(), json!(true));
            }
            vec![message]
        }
        ModelResponseStreamEvent::PartEnd(part) => project_stream_part_end(
            context,
            sequence,
            run_id,
            part.index,
            part.part_kind.as_deref(),
        ),
        ModelResponseStreamEvent::FinalResult(response) => {
            project_model_response(context, sequence, &run_id, response)
        }
    }
}

pub(super) fn project_model_response(
    context: &DisplayProjectionContext,
    sequence: usize,
    run_id: &RunId,
    response: &ModelResponse,
) -> Vec<DisplayMessage> {
    let mut messages = Vec::new();
    for (part_index, part) in response.parts.iter().enumerate() {
        match part {
            ModelResponsePart::Text { text } | ModelResponsePart::ProviderText { text, .. }
                if !text.is_empty() =>
            {
                messages.extend(project_text_response_messages(
                    context, sequence, run_id, part_index, text,
                ));
            }
            ModelResponsePart::Thinking { text, signature }
            | ModelResponsePart::ProviderThinking {
                text, signature, ..
            } if !text.is_empty() => {
                messages.extend(project_thinking_response_messages(
                    context,
                    sequence,
                    run_id,
                    part_index,
                    text,
                    signature.as_ref(),
                ));
            }
            ModelResponsePart::ToolCall(call)
            | ModelResponsePart::ProviderToolCall { call, .. } => {
                messages.extend(project_tool_call_messages(
                    context,
                    sequence,
                    run_id.clone(),
                    call,
                    true,
                ));
            }
            _ => {}
        }
    }
    messages
}

fn project_stream_part_start(
    context: &DisplayProjectionContext,
    sequence: usize,
    run_id: RunId,
    part_index: usize,
    part_kind: &str,
) -> Vec<DisplayMessage> {
    if is_tool_stream_part_kind(part_kind) {
        return Vec::new();
    }
    vec![DisplayMessage::new(
        sequence,
        context.session_id.clone(),
        run_id,
        DisplayMessageKind::AssistantTextStart,
    )
    .with_payload(json!({
        "message_id": format!("message-{part_index}"),
        "role": "assistant",
        "part_index": part_index,
        "part_kind": part_kind,
    }))]
}

fn project_stream_part_end(
    context: &DisplayProjectionContext,
    sequence: usize,
    run_id: RunId,
    part_index: usize,
    part_kind: Option<&str>,
) -> Vec<DisplayMessage> {
    if part_kind.is_some_and(is_tool_stream_part_kind) {
        return Vec::new();
    }
    vec![DisplayMessage::new(
        sequence,
        context.session_id.clone(),
        run_id,
        DisplayMessageKind::AssistantTextEnd,
    )
    .with_payload(json!({
        "message_id": format!("message-{part_index}"),
        "part_index": part_index,
        "part_kind": part_kind,
    }))]
}

fn stream_delta_message_payload(
    part_index: usize,
    delta: &StreamDelta,
) -> Option<(DisplayMessageKind, serde_json::Value)> {
    match delta {
        StreamDelta::ToolCallArguments { .. } | StreamDelta::ToolCallName { .. } => None,
        StreamDelta::Thinking { text } => Some((
            DisplayMessageKind::AssistantTextDelta,
            json!({
                "message_id": format!("message-{part_index}"),
                "part_index": part_index,
                "part_kind": "thinking",
                "delta": text,
            }),
        )),
        StreamDelta::Text { text } => Some((
            DisplayMessageKind::AssistantTextDelta,
            json!({
                "message_id": format!("message-{part_index}"),
                "part_index": part_index,
                "part_kind": "text",
                "delta": text,
            }),
        )),
        StreamDelta::NativePayload { payload } => Some((
            DisplayMessageKind::AssistantTextDelta,
            json!({
                "message_id": format!("message-{part_index}"),
                "part_index": part_index,
                "part_kind": "native_payload",
                "delta": payload.to_string(),
            }),
        )),
        StreamDelta::FileMetadata { payload } => Some((
            DisplayMessageKind::AssistantTextDelta,
            json!({
                "message_id": format!("message-{part_index}"),
                "part_index": part_index,
                "part_kind": "file_metadata",
                "delta": payload.to_string(),
            }),
        )),
    }
}

fn is_tool_stream_part_kind(part_kind: &str) -> bool {
    let normalized = part_kind.to_ascii_lowercase();
    normalized.contains("tool") || normalized.contains("function_call")
}

fn project_text_response_messages(
    context: &DisplayProjectionContext,
    sequence: usize,
    run_id: &RunId,
    part_index: usize,
    text: &str,
) -> Vec<DisplayMessage> {
    let message_id = format!("model-response-{sequence}-{part_index}");
    vec![
        DisplayMessage::new(
            sequence,
            context.session_id.clone(),
            run_id.clone(),
            DisplayMessageKind::AssistantTextStart,
        )
        .with_payload(json!({
            "message_id": message_id,
            "role": "assistant",
            "part_index": part_index,
            "part_kind": "text",
        })),
        DisplayMessage::new(
            sequence,
            context.session_id.clone(),
            run_id.clone(),
            DisplayMessageKind::AssistantTextDelta,
        )
        .with_payload(json!({
            "message_id": message_id,
            "part_index": part_index,
            "part_kind": "text",
            "delta": text,
        }))
        .with_preview(text.to_string()),
        DisplayMessage::new(
            sequence,
            context.session_id.clone(),
            run_id.clone(),
            DisplayMessageKind::AssistantTextEnd,
        )
        .with_payload(json!({
            "message_id": message_id,
            "part_index": part_index,
        })),
    ]
}

fn project_thinking_response_messages(
    context: &DisplayProjectionContext,
    sequence: usize,
    run_id: &RunId,
    part_index: usize,
    text: &str,
    signature: Option<&String>,
) -> Vec<DisplayMessage> {
    let message_id = format!("thinking-{sequence}-{part_index}");
    let has_signature = signature.is_some();
    let mut start = DisplayMessage::new(
        sequence,
        context.session_id.clone(),
        run_id.clone(),
        DisplayMessageKind::AssistantTextStart,
    )
    .with_payload(json!({
        "message_id": message_id,
        "role": "reasoning",
        "part_index": part_index,
        "part_kind": "thinking",
        "has_signature": has_signature,
    }));
    start.metadata.insert("reasoning".to_string(), json!(true));

    let mut delta = DisplayMessage::new(
        sequence,
        context.session_id.clone(),
        run_id.clone(),
        DisplayMessageKind::AssistantTextDelta,
    )
    .with_payload(json!({
        "message_id": message_id,
        "part_index": part_index,
        "part_kind": "thinking",
        "delta": text,
        "thinking": text,
        "has_signature": has_signature,
    }))
    .with_preview(text.to_string());
    delta.metadata.insert("reasoning".to_string(), json!(true));

    let mut end = DisplayMessage::new(
        sequence,
        context.session_id.clone(),
        run_id.clone(),
        DisplayMessageKind::AssistantTextEnd,
    )
    .with_payload(json!({
        "message_id": message_id,
        "part_index": part_index,
        "part_kind": "thinking",
    }));
    end.metadata.insert("reasoning".to_string(), json!(true));

    vec![start, delta, end]
}
