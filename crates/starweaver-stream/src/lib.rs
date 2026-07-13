#![allow(clippy::significant_drop_tightening)]

//! Typed raw execution, display, and replay stream contracts for Starweaver.

mod adapters;
mod archive;
mod compaction;
mod display;
mod envelope;
mod error;
mod raw;
mod replay;
mod sanitizer;
mod transport;

pub use adapters::{
    AguiEvent, VercelDataStreamPart, display_to_agui_event, display_to_agui_jsonl,
    display_to_vercel_data_stream, display_to_vercel_data_stream_jsonl,
};
pub use archive::{InMemoryStreamArchive, StreamArchive, StreamArchiveRecord};
pub use compaction::RealtimeCompactionBuffer;
pub use display::{
    DefaultDisplayMessageProjector, DisplayMessage, DisplayMessageKind, DisplayMessageProjector,
    DisplayProjectionContext, DisplayVisibility,
};
pub use envelope::{JsonlEnvelope, ReplayEnvelope, SseEnvelope};
pub use error::{ReplayError, ReplayResult};
pub use raw::{
    AgentSidebandEvent, AgentSidebandEventCategory, AgentStreamEvent, AgentStreamRecord,
    AgentStreamSink, AgentStreamSource, AgentStreamSourceKind,
};
pub use replay::{
    EnvironmentLifecycleEvent, InMemoryReplayEventLog, ReplayCatchupSource, ReplayCursor,
    ReplayCursorFamily, ReplayEvent, ReplayEventKind, ReplayEventLog, ReplayScope, ReplaySnapshot,
    ReplaySubscription, StreamTerminalMarker,
};
pub use sanitizer::{
    ClientHistorySanitizerConfig, ClientHistoryTrust, SanitizedClientHistory, SanitizerDecision,
    sanitize_client_history,
};
pub use starweaver_core::SessionId;
pub use transport::{
    InMemoryReplayTransport, ReplaySseFrame, ReplayTransport, replay_sse_event_name,
    replay_sse_frames,
};

#[cfg(test)]
mod tests;
