//! Runtime trace recording contract and deterministic in-memory recorder.

mod memory;
mod otel;
mod policy;
mod recorder;
#[cfg(test)]
mod tests;
mod types;

pub use memory::{AdapterTraceRecorder, InMemoryTraceRecorder};
pub use otel::{export_otel_gen_ai_spans, OtelGenAiSpan};
pub use policy::{PolicyTraceRecorder, TraceDebugPolicy, TraceRedactionPolicy};
pub use recorder::{DynTraceRecorder, NoopTraceRecorder, TraceRecorder, TraceRecorderHandle};
pub use types::{RecordedSpan, SpanEvent, SpanHandle, SpanKind, SpanSpec, SpanStatus, TraceLevel};
