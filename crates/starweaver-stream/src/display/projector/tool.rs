//! Tool and handoff display projection.

use serde_json::{Value, json};
use starweaver_core::RunId;
use starweaver_model::{ToolCallPart, ToolReturnPart};

use super::super::custom::payload_string;
use super::super::{DisplayMessage, DisplayMessageKind, DisplayProjectionContext};

pub(super) fn project_tool_call_messages(
    context: &DisplayProjectionContext,
    sequence: usize,
    run_id: RunId,
    call: &ToolCallPart,
    include_end: bool,
) -> Vec<DisplayMessage> {
    let mut messages = Vec::new();
    if call.name == "summarize" {
        messages.push(project_handoff_started(
            context,
            sequence,
            run_id.clone(),
            call,
        ));
    }
    messages.extend([
        project_tool_call_start(context, sequence, run_id.clone(), call),
        project_tool_call_delta(context, sequence, run_id.clone(), call),
    ]);
    if include_end {
        messages.push(project_tool_call_end(context, sequence, run_id, call));
    }
    messages
}

pub(super) fn project_tool_return_messages(
    context: &DisplayProjectionContext,
    sequence: usize,
    run_id: RunId,
    tool_return: &ToolReturnPart,
) -> Vec<DisplayMessage> {
    let tool_result = project_tool_return(context, sequence, run_id.clone(), tool_return);
    if tool_return.name != "summarize" {
        return vec![tool_result];
    }
    let handoff = project_handoff_from_summarize_return(context, sequence, run_id, tool_return);
    vec![tool_result, handoff]
}

fn project_handoff_started(
    context: &DisplayProjectionContext,
    sequence: usize,
    run_id: RunId,
    call: &ToolCallPart,
) -> DisplayMessage {
    let payload = call.arguments.replay_value();
    DisplayMessage::new(
        sequence,
        context.session_id.clone(),
        run_id,
        DisplayMessageKind::HandoffStarted,
    )
    .with_payload(payload)
    .with_preview("handoff summary started")
}

fn project_tool_call_start(
    context: &DisplayProjectionContext,
    sequence: usize,
    run_id: RunId,
    call: &ToolCallPart,
) -> DisplayMessage {
    DisplayMessage::new(
        sequence,
        context.session_id.clone(),
        run_id,
        DisplayMessageKind::ToolCallStart,
    )
    .with_payload(json!({
        "tool_call_id": call.id,
        "tool_name": call.name,
        "name": call.name,
    }))
    .with_preview(format!("tool call {}", call.name))
}

fn project_tool_call_delta(
    context: &DisplayProjectionContext,
    sequence: usize,
    run_id: RunId,
    call: &ToolCallPart,
) -> DisplayMessage {
    DisplayMessage::new(
        sequence,
        context.session_id.clone(),
        run_id,
        DisplayMessageKind::ToolCallDelta,
    )
    .with_payload(json!({
        "tool_call_id": call.id,
        "tool_name": call.name,
        "name": call.name,
        "arguments": call.arguments.replay_value(),
        "delta": call.arguments.wire_json_string(),
    }))
    .with_preview(call.arguments.wire_json_string())
}

fn project_tool_call_end(
    context: &DisplayProjectionContext,
    sequence: usize,
    run_id: RunId,
    call: &ToolCallPart,
) -> DisplayMessage {
    DisplayMessage::new(
        sequence,
        context.session_id.clone(),
        run_id,
        DisplayMessageKind::ToolCallEnd,
    )
    .with_payload(json!({
        "tool_call_id": call.id,
        "tool_name": call.name,
        "name": call.name,
    }))
    .with_preview(format!("tool call {} ended", call.name))
}

fn project_tool_return(
    context: &DisplayProjectionContext,
    sequence: usize,
    run_id: RunId,
    tool_return: &ToolReturnPart,
) -> DisplayMessage {
    let mut payload = serde_json::Map::new();
    payload.insert("tool_call_id".to_string(), json!(tool_return.tool_call_id));
    payload.insert("tool_name".to_string(), json!(tool_return.name));
    payload.insert("content".to_string(), tool_return.content.clone());
    payload.insert("is_error".to_string(), json!(tool_return.is_error));
    if let Some(user_content) = &tool_return.user_content {
        payload.insert("user_content".to_string(), user_content.clone());
    }
    if let Some(app_value) = &tool_return.app_value {
        payload.insert("app_value".to_string(), app_value.clone());
    }
    if !tool_return.metadata.is_empty() {
        payload.insert(
            "metadata".to_string(),
            serde_json::Value::Object(tool_return.metadata.clone()),
        );
    }
    DisplayMessage::new(
        sequence,
        context.session_id.clone(),
        run_id,
        DisplayMessageKind::ToolResult,
    )
    .with_payload(serde_json::Value::Object(payload))
    .with_preview(format!("tool result {}", tool_return.name))
}

fn project_handoff_from_summarize_return(
    context: &DisplayProjectionContext,
    sequence: usize,
    run_id: RunId,
    tool_return: &ToolReturnPart,
) -> DisplayMessage {
    let payload =
        summarize_payload(&tool_return.content).unwrap_or_else(|| tool_return.content.clone());
    let display_kind = if tool_return.is_error {
        DisplayMessageKind::HandoffFailed
    } else {
        DisplayMessageKind::HandoffCompleted
    };
    let preview = if tool_return.is_error {
        payload_string(&payload, &["error", "message", "reason"]).map_or_else(
            || "handoff summary failed".to_string(),
            |error| format!("handoff summary failed: {error}"),
        )
    } else {
        "handoff summary completed".to_string()
    };
    DisplayMessage::new(sequence, context.session_id.clone(), run_id, display_kind)
        .with_payload(payload)
        .with_preview(preview)
}

fn summarize_payload(value: &Value) -> Option<Value> {
    if value.get("operation").and_then(Value::as_str) == Some("summarize") {
        return value.get("payload").cloned();
    }
    None
}
