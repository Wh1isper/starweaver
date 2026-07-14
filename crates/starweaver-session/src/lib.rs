#![allow(clippy::significant_drop_tightening)]

//! Shared durable session contracts for Starweaver.

mod approval;
mod claim;
mod error;
mod evidence;
mod input;
mod management;
mod publication;
mod records;
mod resume;
mod search;
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
pub use management::{
    AcquireRunAdmission, AgentDisplayPage, AgentReplayQuery, AgentRunListQuery, AgentRunPage,
    AgentRunView, AgentSessionControlError, AgentSessionControlErrorCode, AgentSessionInclude,
    AgentSessionListQuery, AgentSessionOperation, AgentSessionPage, AgentSessionQueryError,
    AgentSessionQueryErrorCode, AgentSessionScope, AgentSessionView, CreateManagedSession,
    DeleteManagedSession, DurableControlReceipt, InterruptManagedRun, LOCAL_SESSION_NAMESPACE,
    ManagedRunTarget, ManagedSessionPatch, ManagedSessionTarget, RunAdmissionLease,
    RunAdmissionReceipt, RunControlReceipt, RunStartReceipt, SessionContinuationFence,
    SessionDeletionFence, SessionMutationReceipt, StartManagedRun, SteerManagedRun,
    UpdateManagedSession,
};
pub use publication::{
    PendingStreamPublication, StreamPublicationTarget, StreamPublicationTargets,
};
pub use records::{
    CheckpointRef, DurableRunStatus, EnvironmentStateRef, ExecutionStatus, QueuedRunStatus,
    RunRecord, RunStatus, SessionRecord, SessionStatus, StreamCursorRef, StreamCursorRefError,
};
pub use resume::SessionResumeSnapshot;
pub use search::{
    SessionSearchCapabilities, SessionSearchCheckpoint, SessionSearchConsistency,
    SessionSearchCoverage, SessionSearchCoverageState, SessionSearchCursorBinding,
    SessionSearchCursorCodec, SessionSearchDocument, SessionSearchError, SessionSearchFilter,
    SessionSearchFilterKind, SessionSearchGranularity, SessionSearchHighlight, SessionSearchHit,
    SessionSearchIndexError, SessionSearchIndexWriter, SessionSearchLocation,
    SessionSearchMutation, SessionSearchMutationOperation, SessionSearchPage,
    SessionSearchProvider, SessionSearchQuery, SessionSearchQueryMode, SessionSearchScope,
    SessionSearchSnippet, SessionSearchSort, SessionSearchSource, SessionSearchSummary,
    SessionSearchTimeRange, SessionSearchVisibility, SessionSearchWarning,
    SessionSearchWarningKind,
};
pub use starweaver_core::SessionId;
pub use store::{InMemorySessionStore, SessionFilter, SessionStore, SessionStoreExecutor};
pub use trace::{CompactRunTrace, CompactSessionTrace};

#[cfg(test)]
mod tests;
