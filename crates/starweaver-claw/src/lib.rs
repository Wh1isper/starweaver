//! Durable orchestration foundations for Starweaver.
//!
//! Claw composes the shared session and stream contracts and will host concrete
//! `SQLite`, `PostgreSQL`, `SSE`, `Redis Stream`, and service coordinator adapters.

pub use starweaver_session::{
    ApprovalDecision, ApprovalRecord, ApprovalStatus, BinaryRef, CheckpointRef, CompactRunTrace,
    CompactSessionTrace, DeferredToolRecord, EnvironmentStateRef, ExecutionStatus, FileRef,
    InMemorySessionStore, InputPart, RunRecord, RunStatus, SessionFilter, SessionId, SessionRecord,
    SessionResumeSnapshot, SessionStatus, SessionStore, SessionStoreError, SessionStoreExecutor,
    SessionStoreResult, StreamCursorRef,
};
pub use starweaver_stream::{
    DefaultDisplayMessageProjector, DisplayMessage, DisplayMessageKind, DisplayMessageProjector,
    DisplayProjectionContext, DisplayVisibility, InMemoryReplayEventLog, InMemoryReplayTransport,
    InMemoryStreamArchive, JsonlEnvelope, RealtimeCompactionBuffer, ReplayCursor, ReplayEnvelope,
    ReplayError, ReplayEvent, ReplayEventKind, ReplayEventLog, ReplayResult, ReplayScope,
    ReplaySnapshot, ReplaySubscription, ReplayTransport, SseEnvelope, StreamArchive,
    StreamArchiveRecord, StreamTerminalMarker,
};
