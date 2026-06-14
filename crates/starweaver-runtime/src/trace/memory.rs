use std::sync::{Arc, Mutex};

use starweaver_core::{Metadata, TraceContext};
use uuid::Uuid;

use super::{RecordedSpan, SpanEvent, SpanHandle, SpanSpec, SpanStatus, TraceRecorder};

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
        SpanHandle::new(context, span_id)
    }

    fn record_event(&self, span: &SpanHandle, event: SpanEvent) {
        self.update_span(span.span_id(), |record| record.events.push(event));
    }

    fn close_span(&self, span: &SpanHandle, status: SpanStatus) {
        self.update_span(span.span_id(), |record| record.status = status);
    }
}
