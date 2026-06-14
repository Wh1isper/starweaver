//! Runtime trace recording contract and deterministic in-memory recorder.

mod memory;
mod recorder;
#[cfg(test)]
mod tests;
mod types;

pub use memory::{AdapterTraceRecorder, InMemoryTraceRecorder};
pub use recorder::{DynTraceRecorder, NoopTraceRecorder, TraceRecorder};
pub use types::{RecordedSpan, SpanEvent, SpanHandle, SpanKind, SpanSpec, SpanStatus, TraceLevel};
