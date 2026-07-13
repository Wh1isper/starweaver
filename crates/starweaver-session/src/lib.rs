#![allow(clippy::significant_drop_tightening)]

//! Shared durable session contracts for Starweaver.

mod approval;
mod claim;
mod error;
mod evidence;
mod input;
mod publication;
mod records;
mod resume;
mod store;
mod trace;

pub use approval::{
    ApprovalDecision, ApprovalRecord, ApprovalStatus, DeferredToolRecord, DeferredToolRequest,
    DeferredToolRequests, DeferredToolResult, DeferredToolResults, ToolApprovalDecision,
    ToolReturnRecordInput,
};
pub use claim::{HitlResumeClaim, HitlResumeClaimState};
pub use error::{SessionStoreError, SessionStoreResult};
pub use evidence::{RelatedRunUpdate, RunEvidenceCommit};
pub use input::{BinaryRef, FileRef, InputConversionError, InputPart};
pub use publication::{
    PendingStreamPublication, StreamPublicationTarget, StreamPublicationTargets,
};
pub use records::{
    CheckpointRef, DurableRunStatus, EnvironmentStateRef, ExecutionStatus, QueuedRunStatus,
    RunRecord, RunStatus, SessionRecord, SessionStatus, StreamCursorRef, StreamCursorRefError,
};
pub use resume::SessionResumeSnapshot;
pub use starweaver_core::SessionId;
pub use store::{InMemorySessionStore, SessionFilter, SessionStore, SessionStoreExecutor};
pub use trace::{CompactRunTrace, CompactSessionTrace};

#[cfg(test)]
mod tests;
