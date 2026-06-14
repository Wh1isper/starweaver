use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use starweaver_core::TraceContext;

/// Trace detail level used by spans and events.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceLevel {
    /// Exported by default.
    #[default]
    Info,
    /// Exported when debug telemetry is enabled.
    Debug,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_default_trace_level(level: &TraceLevel) -> bool {
    matches!(level, TraceLevel::Info)
}

/// Span role compatible with OpenTelemetry span kinds.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SpanKind {
    /// Internal runtime work.
    #[default]
    Internal,
    /// Client call to a remote service.
    Client,
    /// Server-side request handling.
    Server,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_default_span_kind(kind: &SpanKind) -> bool {
    matches!(kind, SpanKind::Internal)
}

/// Span lifecycle status.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SpanStatus {
    /// Span is still open.
    Open,
    /// Span completed successfully.
    Ok,
    /// Span completed with an error type.
    Error {
        /// Error type.
        error_type: String,
    },
}

/// Span event record.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SpanEvent {
    /// Event name.
    pub name: String,
    /// Event detail level.
    #[serde(default, skip_serializing_if = "is_default_trace_level")]
    pub level: TraceLevel,
    /// Event attributes.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attributes: BTreeMap<String, Value>,
}

impl SpanEvent {
    /// Create a span event by name.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            level: TraceLevel::Info,
            attributes: BTreeMap::new(),
        }
    }

    /// Mark the event as debug-level telemetry.
    #[must_use]
    pub const fn debug(mut self) -> Self {
        self.level = TraceLevel::Debug;
        self
    }

    /// Attach one event attribute.
    #[must_use]
    pub fn with_attribute(mut self, key: impl Into<String>, value: Value) -> Self {
        self.attributes.insert(key.into(), value);
        self
    }
}

/// Span start specification.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SpanSpec {
    /// Span name.
    pub name: String,
    /// Span role.
    #[serde(default, skip_serializing_if = "is_default_span_kind")]
    pub kind: SpanKind,
    /// Span detail level.
    #[serde(default, skip_serializing_if = "is_default_trace_level")]
    pub level: TraceLevel,
    /// Span attributes.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attributes: BTreeMap<String, Value>,
}

impl SpanSpec {
    /// Create a span spec by name.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            kind: SpanKind::Internal,
            level: TraceLevel::Info,
            attributes: BTreeMap::new(),
        }
    }

    /// Set the span role.
    #[must_use]
    pub const fn with_kind(mut self, kind: SpanKind) -> Self {
        self.kind = kind;
        self
    }

    /// Mark the span as debug-level telemetry.
    #[must_use]
    pub const fn debug(mut self) -> Self {
        self.level = TraceLevel::Debug;
        self
    }

    /// Attach one attribute.
    #[must_use]
    pub fn with_attribute(mut self, key: impl Into<String>, value: Value) -> Self {
        self.attributes.insert(key.into(), value);
        self
    }
}

/// Recorded span snapshot.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RecordedSpan {
    /// Span id.
    pub span_id: String,
    /// Trace id.
    pub trace_id: String,
    /// Optional parent span id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_span_id: Option<String>,
    /// Span name.
    pub name: String,
    /// Span role.
    #[serde(default, skip_serializing_if = "is_default_span_kind")]
    pub kind: SpanKind,
    /// Span detail level.
    #[serde(default, skip_serializing_if = "is_default_trace_level")]
    pub level: TraceLevel,
    /// Span attributes.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attributes: BTreeMap<String, Value>,
    /// Span events.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<SpanEvent>,
    /// Span status.
    pub status: SpanStatus,
}

/// Active span handle.
#[derive(Clone, Debug)]
pub struct SpanHandle {
    context: TraceContext,
    span_id: String,
}

impl SpanHandle {
    pub(super) fn new(context: TraceContext, span_id: impl Into<String>) -> Self {
        Self {
            context,
            span_id: span_id.into(),
        }
    }

    /// Return the span trace context.
    #[must_use]
    pub const fn context(&self) -> &TraceContext {
        &self.context
    }

    /// Consume the handle into its trace context.
    #[must_use]
    pub fn into_context(self) -> TraceContext {
        self.context
    }

    /// Return span id.
    #[must_use]
    pub fn span_id(&self) -> &str {
        &self.span_id
    }
}
