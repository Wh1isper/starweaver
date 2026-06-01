#![allow(clippy::significant_drop_tightening)]

//! Shared display and replay stream contracts for Starweaver.

mod archive;
mod compaction;
mod display;
mod envelope;
mod error;
mod replay;
mod transport;

pub use archive::{InMemoryStreamArchive, StreamArchive, StreamArchiveRecord};
pub use compaction::RealtimeCompactionBuffer;
pub use display::{
    DefaultDisplayMessageProjector, DisplayMessage, DisplayMessageKind, DisplayMessageProjector,
    DisplayProjectionContext, DisplayVisibility,
};
pub use envelope::{JsonlEnvelope, ReplayEnvelope, SseEnvelope};
pub use error::{ReplayError, ReplayResult};
pub use replay::{
    InMemoryReplayEventLog, ReplayCursor, ReplayEvent, ReplayEventKind, ReplayEventLog,
    ReplayScope, ReplaySnapshot, ReplaySubscription, StreamTerminalMarker,
};
pub use starweaver_core::SessionId;
pub use transport::{InMemoryReplayTransport, ReplayTransport};

#[cfg(test)]
mod tests;
