//! Runtime trace recording contract and deterministic in-memory recorder.

mod adapter;
mod memory;
mod noop;
mod recorder;
#[cfg(test)]
mod tests;
mod types;

pub use adapter::AdapterTraceRecorder;
pub use memory::InMemoryTraceRecorder;
pub use noop::NoopTraceRecorder;
pub use recorder::{DynTraceRecorder, TraceRecorder};
pub use types::{RecordedSpan, SpanEvent, SpanHandle, SpanKind, SpanSpec, SpanStatus, TraceLevel};
