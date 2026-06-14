use starweaver_core::TraceContext;

use super::{
    InMemoryTraceRecorder, RecordedSpan, SpanEvent, SpanHandle, SpanSpec, SpanStatus, TraceRecorder,
};

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
