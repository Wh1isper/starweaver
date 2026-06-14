use starweaver_core::TraceContext;

use super::{SpanEvent, SpanHandle, SpanSpec, SpanStatus, TraceRecorder};

/// No-op trace recorder.
#[derive(Clone, Debug, Default)]
pub struct NoopTraceRecorder;

impl TraceRecorder for NoopTraceRecorder {
    fn start_span(&self, _spec: SpanSpec, parent: &TraceContext) -> SpanHandle {
        SpanHandle::new(parent.clone(), parent.span_id.clone().unwrap_or_default())
    }

    fn record_event(&self, _span: &SpanHandle, _event: SpanEvent) {}

    fn close_span(&self, _span: &SpanHandle, _status: SpanStatus) {}
}
