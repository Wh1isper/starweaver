//! Durable session and run records.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use starweaver_context::ResumableState;
use starweaver_core::{CheckpointId, ConversationId, Metadata, RunId, SessionId, TraceContext};

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

/// Durable run status at the session layer.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    /// Run is accepted and awaiting execution.
    #[default]
    Queued,
    /// Run is actively executing.
    Running,
    /// Run is waiting on approval, deferred work, or resume.
    Waiting,
    /// Run completed successfully.
    Completed,
    /// Run failed.
    Failed,
    /// Run was cancelled or interrupted.
    Cancelled,
}

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

/// Stable reference to a stream replay position.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StreamCursorRef {
    /// Cursor family such as `raw_runtime`, `display`, or `replay_event`.
    pub family: String,
    /// Stream scope string.
    pub scope: String,
    /// Last observed sequence.
    pub sequence: usize,
    /// Optional provider cursor string for non-numeric logs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Cursor metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

impl StreamCursorRef {
    /// Build a cursor reference.
    #[must_use]
    pub fn new(family: impl Into<String>, scope: impl Into<String>, sequence: usize) -> Self {
        Self {
            family: family.into(),
            scope: scope.into(),
            sequence,
            cursor: None,
            created_at: Utc::now(),
            metadata: Metadata::default(),
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
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Last update time.
    pub updated_at: DateTime<Utc>,
    /// Metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
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
    /// User/API/bridge input parts.
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
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Last update time.
    pub updated_at: DateTime<Utc>,
    /// Metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
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
            created_at: now,
            updated_at: now,
            metadata: Metadata::default(),
        }
    }
}
