#![allow(clippy::significant_drop_tightening)]

//! Shared display and replay stream contracts for Starweaver.

mod adapters;
mod archive;
mod compaction;
mod display;
mod envelope;
mod error;
mod replay;
mod sanitizer;
mod transport;

pub use adapters::{
    display_to_agui_event, display_to_agui_jsonl, display_to_vercel_data_stream,
    display_to_vercel_data_stream_jsonl, AguiEvent, VercelDataStreamPart,
};
pub use archive::{InMemoryStreamArchive, StreamArchive, StreamArchiveRecord};
pub use compaction::RealtimeCompactionBuffer;
pub use display::{
    DefaultDisplayMessageProjector, DisplayMessage, DisplayMessageKind, DisplayMessageProjector,
    DisplayProjectionContext, DisplayVisibility,
};
pub use envelope::{JsonlEnvelope, ReplayEnvelope, SseEnvelope};
pub use error::{ReplayError, ReplayResult};
pub use replay::{
    EnvironmentLifecycleEvent, InMemoryReplayEventLog, ReplayCursor, ReplayEvent, ReplayEventKind,
    ReplayEventLog, ReplayScope, ReplaySnapshot, ReplaySubscription, StreamTerminalMarker,
};
pub use sanitizer::{
    sanitize_client_history, ClientHistorySanitizerConfig, ClientHistoryTrust,
    SanitizedClientHistory, SanitizerDecision,
};
pub use starweaver_core::SessionId;
pub use transport::{
    replay_sse_event_name, replay_sse_frames, InMemoryReplayTransport, ReplaySseFrame,
    ReplayTransport,
};

#[cfg(test)]
mod tests;
