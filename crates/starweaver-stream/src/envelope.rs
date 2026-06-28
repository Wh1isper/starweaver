//! Transport protocol envelopes.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::replay::{ReplayCursor, ReplayEvent, ReplayEventKind};

/// Generic replay transport envelope.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "protocol", rename_all = "snake_case")]
pub enum ReplayEnvelope {
    /// Server-sent event envelope.
    Sse(SseEnvelope),
    /// JSON Lines envelope.
    Jsonl(JsonlEnvelope),
}

/// SSE frame data.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SseEnvelope {
    /// SSE event id.
    pub id: String,
    /// SSE event name.
    pub event: String,
    /// JSON data payload.
    pub data: Value,
    /// Replay cursor carried with the frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<ReplayCursor>,
}

/// JSONL frame data.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct JsonlEnvelope {
    /// Replay sequence.
    pub sequence: usize,
    /// Event kind name.
    pub kind: String,
    /// JSON data payload.
    pub data: Value,
    /// Replay cursor carried with the frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<ReplayCursor>,
}

impl SseEnvelope {
    /// Build an SSE envelope from a replay event.
    #[must_use]
    pub fn from_event(event: &ReplayEvent) -> Self {
        Self {
            id: event.sequence.to_string(),
            event: event_name(&event.event).to_string(),
            data: event_data(&event.event),
            cursor: Some(ReplayCursor::new(event.scope.clone(), event.sequence)),
        }
    }

    /// Render an SSE frame.
    #[must_use]
    pub fn to_frame(&self) -> String {
        format!(
            "id: {}\nevent: {}\ndata: {}\n\n",
            self.id, self.event, self.data
        )
    }
}

impl JsonlEnvelope {
    /// Build a JSONL envelope from a replay event.
    #[must_use]
    pub fn from_event(event: &ReplayEvent) -> Self {
        Self {
            sequence: event.sequence,
            kind: event_name(&event.event).to_string(),
            data: event_data(&event.event),
            cursor: Some(ReplayCursor::new(event.scope.clone(), event.sequence)),
        }
    }

    /// Render one JSONL line.
    ///
    /// # Errors
    ///
    /// Returns a serialization error when the envelope payload cannot be encoded as JSON.
    pub fn to_line(&self) -> serde_json::Result<String> {
        serde_json::to_string(self).map(|line| format!("{line}\n"))
    }
}

const fn event_name(kind: &ReplayEventKind) -> &'static str {
    match kind {
        ReplayEventKind::DisplayMessage(_) => "display_message",
        ReplayEventKind::EnvironmentLifecycle(_) => "environment_lifecycle",
        ReplayEventKind::Raw(_) => "raw",
        ReplayEventKind::Snapshot(_) => "snapshot",
        ReplayEventKind::Heartbeat => "heartbeat",
        ReplayEventKind::Terminal { .. } => "terminal",
    }
}

fn event_data(kind: &ReplayEventKind) -> Value {
    match kind {
        ReplayEventKind::DisplayMessage(message) => {
            serde_json::to_value(message).unwrap_or(Value::Null)
        }
        ReplayEventKind::EnvironmentLifecycle(event) => {
            serde_json::to_value(event).unwrap_or(Value::Null)
        }
        ReplayEventKind::Raw(value) => value.clone(),
        ReplayEventKind::Snapshot(snapshot) => {
            serde_json::to_value(snapshot).unwrap_or(Value::Null)
        }
        ReplayEventKind::Heartbeat => Value::Null,
        ReplayEventKind::Terminal { marker } => serde_json::to_value(marker).unwrap_or(Value::Null),
    }
}
