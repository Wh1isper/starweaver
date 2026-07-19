#![allow(clippy::significant_drop_tightening)]

//! Shared durable session contracts for Starweaver.

mod approval;
mod background;
mod claim;
mod continuation;
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
pub use background::{
    AcquireBackgroundSubagentContinuation, BACKGROUND_SUBAGENT_RECORD_VERSION,
    BackgroundSubagentArtifact, BackgroundSubagentArtifactLimits,
    BackgroundSubagentContinuationCause, BackgroundSubagentContinuationReceipt,
    BackgroundSubagentRecord, BackgroundSubagentTerminalCommit,
    DEFAULT_BACKGROUND_RESULT_RETENTION_SECS, DurableBackgroundSubagentDeliveryClaim,
    DurableBackgroundSubagentDeliveryRelease, DurableBackgroundSubagentDeliveryStatus,
    DurableBackgroundSubagentExecutionStatus, DurableBackgroundSubagentOwnerLease,
    DurableBackgroundSubagentResultRef, DurableBackgroundSubagentRetentionStatus,
    background_subagent_input_digest, background_subagent_result_digest,
};
pub use claim::{
    CONTINUATION_EFFECT_METADATA_KEY, ContinuationEffectOutcome, ContinuationEffectPhase,
    ContinuationEffectState, HitlResumeAbortOutcome, HitlResumeClaim, HitlResumeClaimState,
};
pub use continuation::{
    ContinuationPreparationError, ContinuationPreparationMode, PreparedContinuation,
};
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
