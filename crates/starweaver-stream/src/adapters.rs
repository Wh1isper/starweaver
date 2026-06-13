//! UI adapter projections over Starweaver display messages.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{DisplayMessage, DisplayMessageKind};

/// AG-UI / Starweaver-compatible top-level event object.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AguiEvent {
    /// Event type such as `RUN_STARTED` or `TEXT_MESSAGE_CONTENT`.
    #[serde(rename = "type")]
    pub event_type: String,
    /// Monotonic event id.
    pub id: String,
    /// Starweaver sequence.
    pub sequence: usize,
    /// Session id.
    pub session_id: String,
    /// Run id.
    pub run_id: String,
    /// Event payload.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub payload: Value,
}

impl AguiEvent {
    /// Render one AG-UI JSONL line.
    ///
    /// # Errors
    ///
    /// Returns a serialization error when the event cannot be encoded.
    pub fn to_jsonl_line(&self) -> serde_json::Result<String> {
        serde_json::to_string(self).map(|line| format!("{line}\n"))
    }
}

/// Vercel AI Data Stream-style part.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct VercelDataStreamPart {
    /// Part type.
    #[serde(rename = "type")]
    pub part_type: String,
    /// Part value.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub value: Value,
}

/// Convert one Starweaver display message into an AG-UI / Starweaver top-level event.
#[must_use]
pub fn display_to_agui_event(message: &DisplayMessage) -> AguiEvent {
    AguiEvent {
        event_type: display_event_type(message.kind).to_string(),
        id: message.sequence.to_string(),
        sequence: message.sequence,
        session_id: message.session_id.as_str().to_string(),
        run_id: message.run_id.as_str().to_string(),
        payload: display_payload(message),
    }
}

/// Convert display messages into AG-UI JSONL.
///
/// # Errors
///
/// Returns a serialization error when any event cannot be encoded.
pub fn display_to_agui_jsonl(messages: &[DisplayMessage]) -> serde_json::Result<String> {
    let mut out = String::new();
    for message in messages {
        out.push_str(&display_to_agui_event(message).to_jsonl_line()?);
    }
    Ok(out)
}

/// Convert one Starweaver display message into Vercel AI Data Stream-style parts.
#[must_use]
pub fn display_to_vercel_data_stream(message: &DisplayMessage) -> Vec<VercelDataStreamPart> {
    match message.kind {
        DisplayMessageKind::RunStarted => vec![part(
            "start",
            json!({
                "runId": message.run_id.as_str(),
                "sessionId": message.session_id.as_str(),
            }),
        )],
        DisplayMessageKind::AssistantTextStart => vec![part("text-start", Value::Null)],
        DisplayMessageKind::AssistantTextDelta => vec![part(
            "text-delta",
            json!({"textDelta": text_delta(&message.payload)}),
        )],
        DisplayMessageKind::AssistantTextEnd => vec![part("text-end", Value::Null)],
        DisplayMessageKind::ToolCallStart => vec![part("tool-call-start", message.payload.clone())],
        DisplayMessageKind::ToolCallDelta => vec![part("tool-call-delta", message.payload.clone())],
        DisplayMessageKind::ToolCallEnd => vec![part("tool-call-end", message.payload.clone())],
        DisplayMessageKind::ToolResult => vec![part("tool-result", message.payload.clone())],
        DisplayMessageKind::RunCompleted => vec![part("finish", message.payload.clone())],
        DisplayMessageKind::RunFailed => vec![part("error", message.payload.clone())],
        DisplayMessageKind::RunCancelled => vec![part("abort", message.payload.clone())],
        DisplayMessageKind::RunQueued
        | DisplayMessageKind::ApprovalRequested
        | DisplayMessageKind::ApprovalResolved
        | DisplayMessageKind::Checkpoint
        | DisplayMessageKind::SubagentStarted
        | DisplayMessageKind::SubagentCompleted
        | DisplayMessageKind::CompactionStarted
        | DisplayMessageKind::CompactionCompleted
        | DisplayMessageKind::CompactionFailed
        | DisplayMessageKind::HandoffStarted
        | DisplayMessageKind::HandoffCompleted
        | DisplayMessageKind::HandoffFailed
        | DisplayMessageKind::SteeringSubmitted
        | DisplayMessageKind::SteeringReceived
        | DisplayMessageKind::TaskSnapshot => vec![part(
            "data",
            json!({
                "type": display_event_type(message.kind),
                "payload": message.payload,
            }),
        )],
    }
}

/// Convert display messages into newline-delimited Vercel AI Data Stream-style JSON parts.
///
/// # Errors
///
/// Returns a serialization error when any part cannot be encoded.
pub fn display_to_vercel_data_stream_jsonl(
    messages: &[DisplayMessage],
) -> serde_json::Result<String> {
    let mut out = String::new();
    for message in messages {
        for part in display_to_vercel_data_stream(message) {
            out.push_str(&serde_json::to_string(&part)?);
            out.push('\n');
        }
    }
    Ok(out)
}

fn display_payload(message: &DisplayMessage) -> Value {
    let mut payload = match &message.payload {
        Value::Object(map) => map.clone(),
        Value::Null => serde_json::Map::new(),
        value => {
            let mut map = serde_json::Map::new();
            map.insert("value".to_string(), value.clone());
            map
        }
    };
    payload.insert(
        "timestamp".to_string(),
        Value::String(message.timestamp.to_rfc3339()),
    );
    if let Some(preview) = &message.preview {
        payload.insert("preview".to_string(), Value::String(preview.clone()));
    }
    Value::Object(payload)
}

const fn display_event_type(kind: DisplayMessageKind) -> &'static str {
    match kind {
        DisplayMessageKind::RunQueued => "RUN_QUEUED",
        DisplayMessageKind::RunStarted => "RUN_STARTED",
        DisplayMessageKind::AssistantTextStart => "TEXT_MESSAGE_START",
        DisplayMessageKind::AssistantTextDelta => "TEXT_MESSAGE_CONTENT",
        DisplayMessageKind::AssistantTextEnd => "TEXT_MESSAGE_END",
        DisplayMessageKind::ToolCallStart => "TOOL_CALL_START",
        DisplayMessageKind::ToolCallDelta => "TOOL_CALL_ARGS",
        DisplayMessageKind::ToolCallEnd => "TOOL_CALL_END",
        DisplayMessageKind::ToolResult => "TOOL_CALL_RESULT",
        DisplayMessageKind::ApprovalRequested => "APPROVAL_REQUESTED",
        DisplayMessageKind::ApprovalResolved => "APPROVAL_RESOLVED",
        DisplayMessageKind::Checkpoint => "CHECKPOINT",
        DisplayMessageKind::SubagentStarted => "SUBAGENT_STARTED",
        DisplayMessageKind::SubagentCompleted => "SUBAGENT_COMPLETED",
        DisplayMessageKind::CompactionStarted => "COMPACTION_STARTED",
        DisplayMessageKind::CompactionCompleted => "COMPACTION_COMPLETED",
        DisplayMessageKind::CompactionFailed => "COMPACTION_FAILED",
        DisplayMessageKind::HandoffStarted => "HANDOFF_STARTED",
        DisplayMessageKind::HandoffCompleted => "HANDOFF_COMPLETED",
        DisplayMessageKind::HandoffFailed => "HANDOFF_FAILED",
        DisplayMessageKind::SteeringSubmitted => "STEERING_SUBMITTED",
        DisplayMessageKind::SteeringReceived => "STEERING_RECEIVED",
        DisplayMessageKind::TaskSnapshot => "TASK_SNAPSHOT",
        DisplayMessageKind::RunCompleted => "RUN_FINISHED",
        DisplayMessageKind::RunFailed => "RUN_ERROR",
        DisplayMessageKind::RunCancelled => "RUN_CANCELLED",
    }
}

fn text_delta(payload: &Value) -> String {
    payload
        .get("delta")
        .or_else(|| payload.get("text_delta"))
        .or_else(|| payload.get("text"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn part(part_type: impl Into<String>, value: Value) -> VercelDataStreamPart {
    VercelDataStreamPart {
        part_type: part_type.into(),
        value,
    }
}
