//! Canonical model stream events.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::message::ModelResponse;

/// Stream event emitted by model adapters.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ModelResponseStreamEvent {
    /// A response part started.
    PartStart(PartStart),
    /// A response part delta arrived.
    PartDelta(PartDelta),
    /// A response part ended.
    PartEnd(PartEnd),
    /// Provider or transport diagnostic sideband event.
    Diagnostic(StreamDiagnostic),
    /// Final response is available.
    FinalResult(Box<ModelResponse>),
}

/// Diagnostic sideband event emitted during model streaming.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StreamDiagnostic {
    /// Stable diagnostic event kind.
    pub kind: String,
    /// Structured diagnostic payload. Must not contain secrets or raw request bodies.
    #[serde(default)]
    pub payload: Value,
    /// Diagnostic metadata.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub metadata: Map<String, Value>,
}

impl StreamDiagnostic {
    /// Build a stream diagnostic event.
    #[must_use]
    pub fn new(kind: impl Into<String>, payload: Value) -> Self {
        Self {
            kind: kind.into(),
            payload,
            metadata: Map::new(),
        }
    }
}

/// Part start event.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PartStart {
    /// Part index in response.
    pub index: usize,
    /// Part kind.
    pub part_kind: String,
}

/// Part delta event.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PartDelta {
    /// Part index in response.
    pub index: usize,
    /// Typed delta payload.
    #[serde(flatten)]
    pub delta: StreamDelta,
}

impl PartDelta {
    /// Build a text delta.
    #[must_use]
    pub fn text(index: usize, text: impl Into<String>) -> Self {
        Self {
            index,
            delta: StreamDelta::Text { text: text.into() },
        }
    }

    /// Build a thinking delta.
    #[must_use]
    pub fn thinking(index: usize, text: impl Into<String>) -> Self {
        Self {
            index,
            delta: StreamDelta::Thinking { text: text.into() },
        }
    }

    /// Return a text-only display projection.
    #[must_use]
    pub fn as_text(&self) -> String {
        match &self.delta {
            StreamDelta::Text { text }
            | StreamDelta::Thinking { text }
            | StreamDelta::ToolCallName { name: text }
            | StreamDelta::ToolCallArguments {
                arguments_delta: text,
            } => text.clone(),
            StreamDelta::NativePayload { payload } | StreamDelta::FileMetadata { payload } => {
                payload.to_string()
            }
        }
    }
}

/// Typed model stream delta payload.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "delta_kind", rename_all = "snake_case")]
pub enum StreamDelta {
    /// Text output delta.
    Text {
        /// Text fragment.
        text: String,
    },
    /// Thinking output delta.
    Thinking {
        /// Thinking fragment.
        text: String,
    },
    /// Tool call name delta.
    ToolCallName {
        /// Tool name fragment or final value.
        name: String,
    },
    /// Tool call argument delta.
    ToolCallArguments {
        /// Argument fragment.
        arguments_delta: String,
    },
    /// Provider-native payload delta.
    NativePayload {
        /// Native payload fragment.
        payload: Value,
    },
    /// File metadata delta.
    FileMetadata {
        /// File metadata fragment.
        payload: Value,
    },
}

/// Part end event.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PartEnd {
    /// Part index in response.
    pub index: usize,
    /// Part kind when the provider includes it or the parser can infer it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part_kind: Option<String>,
}

impl PartEnd {
    /// Build a part end event without a known part kind.
    #[must_use]
    pub const fn new(index: usize) -> Self {
        Self {
            index,
            part_kind: None,
        }
    }

    /// Build a part end event with an explicit part kind.
    #[must_use]
    pub fn with_kind(index: usize, part_kind: impl Into<String>) -> Self {
        Self {
            index,
            part_kind: Some(part_kind.into()),
        }
    }
}

/// Lifecycle state of a streamed model response.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamLifecycle {
    /// Events are still arriving.
    #[default]
    Incomplete,
    /// Final response has been assembled.
    Complete,
    /// Stream ended by explicit cancellation or transport interruption.
    Interrupted,
}

/// Lightweight stream assembly state for replay and lifecycle assertions.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ModelStreamState {
    /// Current lifecycle.
    pub lifecycle: StreamLifecycle,
    /// Number of started parts.
    pub started_parts: usize,
    /// Number of ended parts.
    pub ended_parts: usize,
    /// Final response when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_response: Option<Box<ModelResponse>>,
}

impl ModelStreamState {
    /// Apply one stream event.
    pub fn apply(&mut self, event: &ModelResponseStreamEvent) {
        match event {
            ModelResponseStreamEvent::PartStart(_) => {
                self.started_parts += 1;
            }
            ModelResponseStreamEvent::PartDelta(_) | ModelResponseStreamEvent::Diagnostic(_) => {}
            ModelResponseStreamEvent::PartEnd(_) => {
                self.ended_parts += 1;
            }
            ModelResponseStreamEvent::FinalResult(response) => {
                self.lifecycle = StreamLifecycle::Complete;
                self.final_response = Some(response.clone());
            }
        }
    }

    /// Mark stream as interrupted.
    pub const fn interrupt(&mut self) {
        self.lifecycle = StreamLifecycle::Interrupted;
    }
}
