//! Runtime trace recording contract and deterministic in-memory recorder.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use starweaver_core::{Metadata, TraceContext};
use uuid::Uuid;

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

/// Shared trace recorder reference.
pub type DynTraceRecorder = Arc<dyn TraceRecorder>;

/// Runtime trace recorder abstraction.
pub trait TraceRecorder: Send + Sync {
    /// Start a child span.
    fn start_span(&self, spec: SpanSpec, parent: &TraceContext) -> SpanHandle;

    /// Record a span event.
    fn record_event(&self, span: &SpanHandle, event: SpanEvent);

    /// Close a span.
    fn close_span(&self, span: &SpanHandle, status: SpanStatus);
}

/// No-op trace recorder.
#[derive(Clone, Debug, Default)]
pub struct NoopTraceRecorder;

/// Adapter seam for feature-gated tracing/OpenTelemetry exporters.
#[derive(Clone, Debug, Default)]
pub struct AdapterTraceRecorder {
    inner: InMemoryTraceRecorder,
}

impl AdapterTraceRecorder {
    /// Create an adapter seam backed by deterministic in-memory storage.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Return the spans captured before export.
    #[must_use]
    pub fn spans(&self) -> Vec<RecordedSpan> {
        self.inner.spans()
    }
}

impl TraceRecorder for AdapterTraceRecorder {
    fn start_span(&self, spec: SpanSpec, parent: &TraceContext) -> SpanHandle {
        self.inner.start_span(spec, parent)
    }

    fn record_event(&self, span: &SpanHandle, event: SpanEvent) {
        self.inner.record_event(span, event);
    }

    fn close_span(&self, span: &SpanHandle, status: SpanStatus) {
        self.inner.close_span(span, status);
    }
}

impl TraceRecorder for NoopTraceRecorder {
    fn start_span(&self, _spec: SpanSpec, parent: &TraceContext) -> SpanHandle {
        SpanHandle {
            context: parent.clone(),
            span_id: parent.span_id.clone().unwrap_or_default(),
        }
    }

    fn record_event(&self, _span: &SpanHandle, _event: SpanEvent) {}

    fn close_span(&self, _span: &SpanHandle, _status: SpanStatus) {}
}

/// Deterministic in-memory trace recorder for tests and CLI inspection.
#[derive(Clone, Debug, Default)]
pub struct InMemoryTraceRecorder {
    spans: Arc<Mutex<Vec<RecordedSpan>>>,
}

impl InMemoryTraceRecorder {
    /// Create an empty recorder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Return recorded spans.
    #[must_use]
    pub fn spans(&self) -> Vec<RecordedSpan> {
        self.spans
            .lock()
            .map_or_else(|_| Vec::new(), |spans| spans.clone())
    }

    fn update_span(&self, span_id: &str, f: impl FnOnce(&mut RecordedSpan)) {
        if let Ok(mut spans) = self.spans.lock() {
            if let Some(span) = spans.iter_mut().find(|span| span.span_id == span_id) {
                f(span);
            }
        }
    }
}

impl TraceRecorder for InMemoryTraceRecorder {
    fn start_span(&self, spec: SpanSpec, parent: &TraceContext) -> SpanHandle {
        let trace_id = parent
            .trace_id
            .clone()
            .unwrap_or_else(|| format!("trace_{}", Uuid::new_v4()));
        let parent_span_id = parent
            .span_id
            .clone()
            .or_else(|| parent.parent_span_id.clone());
        let span_id = format!("span_{}", Uuid::new_v4());
        let mut metadata = Metadata::default();
        metadata.insert("span_name".to_string(), serde_json::json!(spec.name));
        let context = TraceContext {
            trace_id: Some(trace_id.clone()),
            span_id: Some(span_id.clone()),
            parent_span_id: parent_span_id.clone(),
            trace_state: parent.trace_state.clone(),
            metadata,
        };
        if let Ok(mut spans) = self.spans.lock() {
            spans.push(RecordedSpan {
                span_id: span_id.clone(),
                trace_id,
                parent_span_id,
                name: spec.name,
                kind: spec.kind,
                level: spec.level,
                attributes: spec.attributes,
                events: Vec::new(),
                status: SpanStatus::Open,
            });
        }
        SpanHandle { context, span_id }
    }

    fn record_event(&self, span: &SpanHandle, event: SpanEvent) {
        self.update_span(span.span_id(), |record| record.events.push(event));
    }

    fn close_span(&self, span: &SpanHandle, status: SpanStatus) {
        self.update_span(span.span_id(), |record| record.status = status);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_memory_recorder_nests_child_spans() {
        let recorder = InMemoryTraceRecorder::new();
        let parent = TraceContext::from_trace_id("trace-test").with_span_id("root");
        let agent = recorder.start_span(SpanSpec::new("gen_ai.invoke_agent"), &parent);
        let model = recorder.start_span(SpanSpec::new("gen_ai.inference"), agent.context());
        recorder.close_span(&model, SpanStatus::Ok);
        recorder.close_span(&agent, SpanStatus::Ok);
        let spans = recorder.spans();
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].parent_span_id.as_deref(), Some("root"));
        assert_eq!(
            spans[1].parent_span_id.as_deref(),
            spans[0].span_id.as_str().into()
        );
    }

    #[test]
    fn span_specs_and_events_capture_kind_level_and_attributes() {
        let recorder = InMemoryTraceRecorder::new();
        let span = recorder.start_span(
            SpanSpec::new("starweaver.filter.all")
                .with_kind(SpanKind::Client)
                .debug()
                .with_attribute("starweaver.filter.name", serde_json::json!("all")),
            &TraceContext::default(),
        );
        recorder.record_event(
            &span,
            SpanEvent::new("starweaver.filter.snapshot")
                .debug()
                .with_attribute("before", serde_json::json!(2))
                .with_attribute("after", serde_json::json!(1)),
        );
        recorder.close_span(
            &span,
            SpanStatus::Error {
                error_type: "filter_failed".to_string(),
            },
        );

        let spans = recorder.spans();
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].kind, SpanKind::Client);
        assert_eq!(spans[0].level, TraceLevel::Debug);
        assert_eq!(
            spans[0].attributes["starweaver.filter.name"],
            serde_json::json!("all")
        );
        assert_eq!(spans[0].events[0].level, TraceLevel::Debug);
        assert_eq!(
            spans[0].events[0].attributes["before"],
            serde_json::json!(2)
        );
        assert!(matches!(
            spans[0].status,
            SpanStatus::Error { ref error_type } if error_type == "filter_failed"
        ));
    }

    #[test]
    fn no_op_recorder_preserves_parent_context() {
        let recorder = NoopTraceRecorder;
        let parent = TraceContext::from_trace_id("trace-noop").with_span_id("parent");
        let span = recorder.start_span(SpanSpec::new("noop"), &parent);
        recorder.record_event(&span, SpanEvent::new("ignored"));
        recorder.close_span(&span, SpanStatus::Ok);

        assert_eq!(span.span_id(), "parent");
        assert_eq!(span.context().trace_id.as_deref(), Some("trace-noop"));
        assert_eq!(span.into_context().span_id.as_deref(), Some("parent"));
    }

    #[test]
    fn adapter_recorder_delegates_to_in_memory_store() {
        let recorder = AdapterTraceRecorder::new();
        let parent = TraceContext::from_trace_id("trace-adapter");
        let span = recorder.start_span(
            SpanSpec::new("gen_ai.inference").with_kind(SpanKind::Client),
            &parent,
        );
        recorder.record_event(&span, SpanEvent::new("starweaver.model.request"));
        recorder.close_span(&span, SpanStatus::Ok);

        let spans = recorder.spans();
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].trace_id, "trace-adapter");
        assert_eq!(spans[0].kind, SpanKind::Client);
        assert_eq!(spans[0].events[0].name, "starweaver.model.request");
    }

    #[test]
    fn in_memory_recorder_ignores_unknown_span_updates() {
        let recorder = InMemoryTraceRecorder::new();
        let unknown = SpanHandle {
            context: TraceContext::from_trace_id("trace-missing"),
            span_id: "missing".to_string(),
        };
        recorder.record_event(&unknown, SpanEvent::new("missing.event"));
        recorder.close_span(&unknown, SpanStatus::Ok);
        assert!(recorder.spans().is_empty());
    }
}
