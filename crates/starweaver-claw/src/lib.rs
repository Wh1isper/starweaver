#![allow(
    clippy::cast_possible_truncation,
    clippy::derive_partial_eq_without_eq,
    clippy::doc_markdown,
    clippy::double_must_use,
    clippy::expect_used,
    clippy::map_unwrap_or,
    clippy::match_same_arms,
    clippy::missing_const_for_fn,
    clippy::missing_errors_doc,
    clippy::format_push_string,
    clippy::module_name_repetitions,
    clippy::needless_pass_by_value,
    clippy::redundant_clone,
    clippy::significant_drop_tightening,
    clippy::struct_excessive_bools,
    clippy::too_many_lines
)]
//! Durable single-node orchestration service for Starweaver.
//!
//! The crate keeps the shared `starweaver-session` and `starweaver-stream`
//! contracts as its foundation and adds the Starweaver Claw product surface:
//! execution profiles, workspace binding, queued run coordination, replayable
//! events, and a local HTTP API.

pub(crate) mod api;
pub mod config;
pub mod controller;
pub mod error;
pub mod execution;
pub mod orchestration;
pub mod profile;
pub mod runtime_state;
pub mod service;
pub mod storage;
pub mod web_assets;
pub mod workspace;

pub use config::{ClawSettings, WorkspaceBackend};
pub use controller::{
    ClawController, ClawInputPart, ClawRunCreateRequest, ClawRunDetail, ClawSessionCreateRequest,
    ClawSessionCreateResponse, ClawSessionForkRequest, ClawSessionGetResponse,
    ClawSessionRunCreateRequest, ClawSessionSummary, ClawTriggerType, DispatchMode,
};
pub use error::{ClawError, ClawResult};
pub use execution::{ExecutionOutput, ExecutionSupervisor, NoopRunExecutor, RunExecutor};
pub use orchestration::{
    HeartbeatStatus, OrchestrationCatalog, ScheduleCreateRequest, ScheduleExecutionMode,
    ScheduleRecord, ScheduleStatus, ScheduleTriggerKind, WorkflowDefinitionCreateRequest,
    WorkflowDefinitionRecord, WorkflowDefinitionStatus, WorkflowRunRecord, WorkflowRunStatus,
    WorkflowScope, WorkflowTriggerKind, WorkflowTriggerRequest,
};
pub use profile::{AgentProfile, ProfileResolver};
pub use runtime_state::{ClawRuntimeState, RuntimeRunHandle};
pub use service::{build_router, serve};
pub use storage::{migrate_sqlite_database, SqliteReplayEventLog, SqliteSessionStore};
pub use workspace::{
    ResolvedWorkspaceBinding, WorkspaceBindingSpec, WorkspaceMountMode, WorkspaceMountSpec,
    WorkspaceProvider,
};

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
