use std::sync::Arc;

use starweaver_core::TraceContext;

use super::{SpanEvent, SpanHandle, SpanSpec, SpanStatus};

/// Shared trace recorder reference.
pub type DynTraceRecorder = Arc<dyn TraceRecorder>;

/// Typed dependency handle for tools that need to create child trace spans.
#[derive(Clone)]
pub struct TraceRecorderHandle {
    recorder: DynTraceRecorder,
}

impl TraceRecorderHandle {
    /// Create a tool dependency handle from a shared recorder.
    #[must_use]
    pub fn new(recorder: DynTraceRecorder) -> Self {
        Self { recorder }
    }

    /// Return the shared recorder.
    #[must_use]
    pub fn recorder(&self) -> DynTraceRecorder {
        self.recorder.clone()
    }
}

impl std::fmt::Debug for TraceRecorderHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("TraceRecorderHandle")
    }
}

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

impl TraceRecorder for NoopTraceRecorder {
    fn start_span(&self, _spec: SpanSpec, parent: &TraceContext) -> SpanHandle {
        SpanHandle::new(parent.clone(), parent.span_id.clone().unwrap_or_default())
    }

    fn record_event(&self, _span: &SpanHandle, _event: SpanEvent) {}

    fn close_span(&self, _span: &SpanHandle, _status: SpanStatus) {}
}
