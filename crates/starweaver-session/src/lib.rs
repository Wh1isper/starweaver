#![allow(clippy::significant_drop_tightening)]

//! Shared durable session contracts for Starweaver.

mod approval;
mod background;
mod claim;
mod continuation;
mod environment;
mod error;
mod evidence;
mod host_events;
mod input;
mod interaction;
mod management;
mod model_selection;
mod publication;
mod records;
mod resume;
mod run_control;
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
pub use environment::{
    AttachEnvironment, DetachEnvironment, DurableEnvironmentAttachment, DurableEnvironmentMount,
    DurableEnvironmentMountStatus, DurableEnvironmentScope, DurableEnvironmentStatus,
    ENVIRONMENT_ATTACH_OPERATION, ENVIRONMENT_DETACH_OPERATION, ENVIRONMENT_MOUNT_OPERATION,
    ENVIRONMENT_UNMOUNT_OPERATION, EnvironmentAttachmentMutationResult, EnvironmentAttachmentPage,
    EnvironmentAttachmentPageKey, EnvironmentAttachmentPageKeyProjection,
    EnvironmentAttachmentQuery, EnvironmentHostEventContext, EnvironmentMountMutationResult,
    EnvironmentMountQuery, EnvironmentMutationContext, EnvironmentMutationResult,
    MAX_ENVIRONMENT_MOUNTS_PER_RUN, MAX_ENVIRONMENT_PAGE_SIZE, MountEnvironmentResource,
    UnmountEnvironmentResource,
};
pub use error::{SessionStoreError, SessionStoreResult};
pub use evidence::{RelatedRunUpdate, RunEvidenceCommit};
pub use host_events::{
    DurableHostEventClass, DurableHostEventPage, DurableHostEventQuery, DurableHostEventRecord,
    DurableHostEventScope, EventPublicationKey, MAX_HOST_EVENT_PAGE_SIZE, MAX_HOST_EVENT_POSITION,
    OutputAvailableProjection, PendingHostEventPublication, RunChangedProjection,
    RunChangedSummary, append_authoritative_run_publications, output_available_publication,
    run_changed_publication,
};
pub use input::{BinaryRef, FileRef, InputConversionError, InputPart};
pub use interaction::{
    APPROVAL_DECIDE_OPERATION, ASK_USER_QUESTION_ACTION, ApprovalMutationResult,
    CLARIFICATION_ANSWERS_METADATA_KEY, CLARIFICATION_RESOLVE_OPERATION,
    CLARIFICATION_RESPONSE_METADATA_KEY, ClarificationAnswer, ClarificationMutationResult,
    ClarificationOption, ClarificationQuestion, ClarificationResolution,
    DEFERRED_COMPLETE_OPERATION, DEFERRED_FAIL_OPERATION, DecideApproval, DeferredMutationOutcome,
    DeferredMutationResult, InteractionMutationContext, ResolveClarification, ResolveDeferredTool,
    validate_clarification_answers,
};
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
pub use model_selection::{
    DurableModelSelection, InitializeModelSelection, MODEL_SELECTION_OPERATION,
    ModelSelectionMutationReceipt, MutationReceipt, SelectModel,
};
pub use publication::{
    PendingStreamPublication, StreamPublicationTarget, StreamPublicationTargets,
};
pub use records::{
    CheckpointRef, DurableRunStatus, EnvironmentStateRef, ExecutionStatus, QueuedRunStatus,
    RunRecord, RunStatus, RunTerminalError, RunTerminalProjection, RunTerminalProjectionError,
    SessionRecord, SessionStatus, StreamCursorRef, StreamCursorRefError,
};
pub use resume::SessionResumeSnapshot;
pub use run_control::{
    AdmitRunControl, DurableRunControlEffect, DurableRunControlIntent, DurableRunControlStatus,
    deterministic_run_control_operation_id, deterministic_run_control_receipt_id,
};
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
pub use store::{
    InMemorySessionStore, InteractionPage, InteractionPageKey, InteractionPageQuery,
    MAX_STABLE_PAGE_SIZE, SessionFilter, SessionPage, SessionPageKey, SessionPageQuery,
    SessionStore, SessionStoreExecutor,
};
pub use trace::{CompactRunTrace, CompactSessionTrace};

#[cfg(test)]
mod tests;
