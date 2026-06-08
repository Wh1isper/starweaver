#![allow(clippy::significant_drop_tightening)]

//! Shared durable session contracts for Starweaver.

mod approval;
mod error;
mod input;
mod records;
mod resume;
mod store;
mod trace;

pub use approval::{
    ApprovalDecision, ApprovalRecord, ApprovalStatus, DeferredToolRecord, DeferredToolRequest,
    DeferredToolRequests, DeferredToolResult, DeferredToolResults, ToolApprovalDecision,
};
pub use error::{SessionStoreError, SessionStoreResult};
pub use input::{BinaryRef, FileRef, InputPart};
pub use records::{
    CheckpointRef, EnvironmentStateRef, ExecutionStatus, RunRecord, RunStatus, SessionRecord,
    SessionStatus, StreamCursorRef,
};
pub use resume::SessionResumeSnapshot;
pub use starweaver_core::SessionId;
pub use store::{InMemorySessionStore, SessionFilter, SessionStore, SessionStoreExecutor};
pub use trace::{CompactRunTrace, CompactSessionTrace};

#[cfg(test)]
mod tests;
