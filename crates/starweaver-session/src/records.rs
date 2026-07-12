//! Durable session and run records.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use serde_json::Value;
use starweaver_context::ResumableState;
use starweaver_core::{
    CheckpointId, ConversationId, Metadata, RunId, RunLifecycle, SessionId, TaskId, TraceContext,
};
use starweaver_stream::{ReplayCursor, ReplayCursorFamily, ReplayScope};

use crate::input::InputPart;

/// Current durable session status.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    /// Session can accept work.
    #[default]
    Active,
    /// Session is archived.
    Archived,
    /// Session reached a failed terminal state.
    Failed,
}

/// Durable run status composed from admission state and the shared runtime lifecycle.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DurableRunStatus(Option<RunLifecycle>);

/// Backward-compatible public name for the durable run status.
pub type RunStatus = DurableRunStatus;

#[allow(non_upper_case_globals)]
impl DurableRunStatus {
    /// Run is accepted and awaiting runtime admission.
    pub const Queued: Self = Self(None);
    /// Runtime initialization is in progress.
    pub const Starting: Self = Self(Some(RunLifecycle::Starting));
    /// Runtime is actively executing.
    pub const Running: Self = Self(Some(RunLifecycle::Running));
    /// Runtime is waiting for external work.
    pub const Waiting: Self = Self(Some(RunLifecycle::Waiting));
    /// Runtime completed successfully.
    pub const Completed: Self = Self(Some(RunLifecycle::Completed));
    /// Runtime failed.
    pub const Failed: Self = Self(Some(RunLifecycle::Failed));
    /// Runtime was cancelled or interrupted.
    pub const Cancelled: Self = Self(Some(RunLifecycle::Cancelled));

    /// Return the admitted runtime lifecycle, or `None` while queued.
    #[must_use]
    pub const fn lifecycle(self) -> Option<RunLifecycle> {
        self.0
    }

    /// Return the stable flat wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self.0 {
            None => "queued",
            Some(lifecycle) => lifecycle.as_str(),
        }
    }

    /// Return whether the run owns the active session slot.
    #[must_use]
    pub const fn is_active(self) -> bool {
        matches!(
            self.0,
            None | Some(RunLifecycle::Starting | RunLifecycle::Running | RunLifecycle::Waiting)
        )
    }

    /// Return whether the run reached a terminal lifecycle.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        match self.0 {
            Some(lifecycle) => lifecycle.is_terminal(),
            None => false,
        }
    }
}

impl From<RunLifecycle> for DurableRunStatus {
    fn from(value: RunLifecycle) -> Self {
        Self(Some(value))
    }
}

impl TryFrom<DurableRunStatus> for RunLifecycle {
    type Error = QueuedRunStatus;

    fn try_from(value: DurableRunStatus) -> Result<Self, Self::Error> {
        value.0.ok_or(QueuedRunStatus)
    }
}

impl Serialize for DurableRunStatus {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for DurableRunStatus {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match String::deserialize(deserializer)?.as_str() {
            "queued" => Ok(Self::Queued),
            "starting" => Ok(Self::Starting),
            "running" => Ok(Self::Running),
            "waiting" => Ok(Self::Waiting),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            other => Err(D::Error::custom(format!("unknown run status: {other}"))),
        }
    }
}

/// A queued durable run has not entered an executable runtime lifecycle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QueuedRunStatus;

impl std::fmt::Display for QueuedRunStatus {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("queued run has no runtime lifecycle")
    }
}

impl std::error::Error for QueuedRunStatus {}

/// Generic execution status for approval, deferred, checkpoint, and archive workflows.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStatus {
    /// Item has been created.
    Pending,
    /// Item is currently processing.
    Running,
    /// Item is waiting on an external decision or worker.
    Waiting,
    /// Item completed successfully.
    Completed,
    /// Item failed.
    Failed,
    /// Item was cancelled.
    Cancelled,
}

/// Stable reference to an exported environment provider state.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct EnvironmentStateRef {
    /// Environment provider name.
    pub provider: String,
    /// Stable provider state reference.
    pub reference: String,
    /// Provider state revision, hash, or generation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    /// Provider-specific metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

/// Stable reference to a persisted checkpoint.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CheckpointRef {
    /// Checkpoint id.
    pub checkpoint_id: CheckpointId,
    /// Run id.
    pub run_id: RunId,
    /// Checkpoint sequence within the run.
    pub sequence: usize,
    /// Runtime node name.
    pub node: String,
    /// Optional storage URI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_ref: Option<String>,
    /// Stream cursor captured with this checkpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_cursor: Option<usize>,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Checkpoint metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

/// Stable durable reference to a family-aware stream replay position.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct StreamCursorRef {
    /// Canonical stream cursor. Family, scope, sequence, and backend position live here once.
    pub position: ReplayCursor,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Cursor metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

impl StreamCursorRef {
    /// Build a durable reference from a canonical cursor.
    #[must_use]
    pub fn new(position: ReplayCursor) -> Self {
        Self {
            position,
            created_at: Utc::now(),
            metadata: Metadata::default(),
        }
    }

    /// Return the cursor family.
    #[must_use]
    pub const fn family(&self) -> ReplayCursorFamily {
        self.position.family
    }

    /// Return the cursor scope.
    #[must_use]
    pub const fn scope(&self) -> &ReplayScope {
        &self.position.scope
    }

    /// Return the last observed sequence.
    #[must_use]
    pub const fn sequence(&self) -> usize {
        self.position.sequence
    }

    /// Return whether two references address the same stream family and scope.
    #[must_use]
    pub fn same_stream(&self, other: &Self) -> bool {
        self.family() == other.family() && self.scope() == other.scope()
    }

    /// Validate that this cursor belongs to the supplied run scope.
    ///
    /// # Errors
    ///
    /// Returns an error when the cursor addresses another run or a non-run scope.
    pub fn validate_for_run(&self, run_id: &RunId) -> Result<(), StreamCursorRefError> {
        let expected = ReplayScope::run(run_id.as_str());
        if self.scope() != &expected {
            return Err(StreamCursorRefError::WrongScope {
                expected: expected.as_str().to_string(),
                actual: self.scope().as_str().to_string(),
            });
        }
        Ok(())
    }

    /// Validate that replacing an existing same-stream cursor does not regress.
    ///
    /// # Errors
    ///
    /// Returns an error when the proposed sequence is behind the current sequence.
    pub fn validate_progression(&self, current: &Self) -> Result<(), StreamCursorRefError> {
        if self.same_stream(current) && self.sequence() < current.sequence() {
            return Err(StreamCursorRefError::SequenceRegression {
                family: self.family(),
                scope: self.scope().as_str().to_string(),
                current: current.sequence(),
                proposed: self.sequence(),
            });
        }
        Ok(())
    }
}

/// Invalid durable stream-cursor update.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StreamCursorRefError {
    /// Cursor scope does not identify the run being updated.
    WrongScope {
        /// Expected run scope.
        expected: String,
        /// Supplied scope.
        actual: String,
    },
    /// Cursor sequence would move durable replay progress backwards.
    SequenceRegression {
        /// Cursor family.
        family: ReplayCursorFamily,
        /// Cursor scope.
        scope: String,
        /// Current sequence.
        current: usize,
        /// Proposed older sequence.
        proposed: usize,
    },
}

impl std::fmt::Display for StreamCursorRefError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WrongScope { expected, actual } => {
                write!(
                    formatter,
                    "expected cursor scope {expected}, received {actual}"
                )
            }
            Self::SequenceRegression {
                family,
                scope,
                current,
                proposed,
            } => write!(
                formatter,
                "{} cursor for {scope} regressed from {current} to {proposed}",
                family.as_str()
            ),
        }
    }
}

impl std::error::Error for StreamCursorRefError {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CurrentStreamCursorRefWire {
    position: ReplayCursor,
    created_at: DateTime<Utc>,
    #[serde(default)]
    metadata: Metadata,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyStreamCursorRefWire {
    family: String,
    scope: String,
    sequence: usize,
    #[serde(default)]
    cursor: Option<String>,
    created_at: DateTime<Utc>,
    #[serde(default)]
    metadata: Metadata,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum StreamCursorRefWire {
    Current(CurrentStreamCursorRefWire),
    Legacy(LegacyStreamCursorRefWire),
}

impl<'de> Deserialize<'de> for StreamCursorRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match StreamCursorRefWire::deserialize(deserializer)? {
            StreamCursorRefWire::Current(current) => Ok(Self {
                position: current.position,
                created_at: current.created_at,
                metadata: current.metadata,
            }),
            StreamCursorRefWire::Legacy(legacy) => {
                let LegacyStreamCursorRefWire {
                    family,
                    scope,
                    sequence,
                    cursor,
                    created_at,
                    metadata,
                } = legacy;
                let family = match family.as_str() {
                    "raw_runtime" => ReplayCursorFamily::RawRuntime,
                    "display" => ReplayCursorFamily::Display,
                    "replay_event" => ReplayCursorFamily::ReplayEvent,
                    other => {
                        return Err(D::Error::custom(format!(
                            "unknown stream cursor family: {other}"
                        )));
                    }
                };
                let mut position =
                    ReplayCursor::for_family(family, ReplayScope::from_string(scope), sequence);
                position.backend_cursor = cursor;
                Ok(Self {
                    position,
                    created_at,
                    metadata,
                })
            }
        }
    }
}

/// Durable session record.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SessionRecord {
    /// Session id.
    pub session_id: SessionId,
    /// User-facing title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Workspace identifier or path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    /// Runtime profile or model profile name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// Session status.
    #[serde(default)]
    pub status: SessionStatus,
    /// Last exported context state.
    #[serde(default)]
    pub state: ResumableState,
    /// Latest exported environment state reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_state: Option<EnvironmentStateRef>,
    /// Latest stream cursor refs by family.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stream_cursors: Vec<StreamCursorRef>,
    /// Session trace context.
    #[serde(default, skip_serializing_if = "TraceContext::is_empty")]
    pub trace_context: TraceContext,
    /// Parent session id for forks or delegated flows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<SessionId>,
    /// Latest run in the session sequence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_run_id: Option<RunId>,
    /// Latest completed run usable as continuation source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_success_run_id: Option<RunId>,
    /// Currently queued, running, or waiting run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_run_id: Option<RunId>,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Last update time.
    pub updated_at: DateTime<Utc>,
    /// Metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

impl starweaver_core::VersionedRecord for SessionRecord {
    const SCHEMA: &'static str = "starweaver.session.session_record";
    const ALLOW_BARE_V0: bool = true;
}

impl SessionRecord {
    /// Build a session record with default state.
    #[must_use]
    pub fn new(session_id: SessionId) -> Self {
        let now = Utc::now();
        Self {
            session_id,
            title: None,
            workspace: None,
            profile: None,
            status: SessionStatus::Active,
            state: ResumableState::default(),
            environment_state: None,
            stream_cursors: Vec::new(),
            trace_context: TraceContext::default(),
            parent_session_id: None,
            head_run_id: None,
            head_success_run_id: None,
            active_run_id: None,
            created_at: now,
            updated_at: now,
            metadata: Metadata::default(),
        }
    }
}

/// Durable run record.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunRecord {
    /// Session id.
    pub session_id: SessionId,
    /// Run id.
    pub run_id: RunId,
    /// Conversation id.
    pub conversation_id: ConversationId,
    /// User, API, or service input parts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub input: Vec<InputPart>,
    /// Durable run status.
    #[serde(default)]
    pub status: RunStatus,
    /// Final output preview.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_preview: Option<String>,
    /// Final structured output preview or summary.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub structured_output: Value,
    /// Latest checkpoint reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_checkpoint: Option<CheckpointRef>,
    /// Latest environment state reference for this run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_state: Option<EnvironmentStateRef>,
    /// Latest stream cursor refs by family.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stream_cursors: Vec<StreamCursorRef>,
    /// Trace context.
    #[serde(default, skip_serializing_if = "TraceContext::is_empty")]
    pub trace_context: TraceContext,
    /// Monotonic order inside the session.
    #[serde(default)]
    pub sequence_no: usize,
    /// Run snapshot used as continuation source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restore_from_run_id: Option<RunId>,
    /// Parent run identifier when this run is delegated from another run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_run_id: Option<RunId>,
    /// Parent-scoped delegated task identifier when this run executes a lightweight task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_task_id: Option<TaskId>,
    /// Trigger source such as cli, service, schedule, or delegated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger_type: Option<String>,
    /// Profile resolved for this run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Last update time.
    pub updated_at: DateTime<Utc>,
    /// Metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

impl starweaver_core::VersionedRecord for RunRecord {
    const SCHEMA: &'static str = "starweaver.session.run_record";
    const ALLOW_BARE_V0: bool = true;
}

impl RunRecord {
    /// Build a run record for a session.
    #[must_use]
    pub fn new(session_id: SessionId, run_id: RunId, conversation_id: ConversationId) -> Self {
        let now = Utc::now();
        Self {
            session_id,
            run_id,
            conversation_id,
            input: Vec::new(),
            status: RunStatus::Queued,
            output_preview: None,
            structured_output: Value::Null,
            latest_checkpoint: None,
            environment_state: None,
            stream_cursors: Vec::new(),
            trace_context: TraceContext::default(),
            sequence_no: 0,
            restore_from_run_id: None,
            parent_run_id: None,
            parent_task_id: None,
            trigger_type: None,
            profile: None,
            created_at: now,
            updated_at: now,
            metadata: Metadata::default(),
        }
    }
}
