use std::sync::Arc;

use starweaver_core::TraceContext;

use super::{SpanEvent, SpanHandle, SpanSpec, SpanStatus};

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
