//! Compact session and run projections.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use starweaver_core::{CheckpointId, Metadata, RunId, SessionId, TaskId, TraceContext};

use crate::records::{RunStatus, SessionStatus, StreamCursorRef};

/// Compact run projection for tools, CLI, and UI.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct CompactRunTrace {
    /// Session id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    /// Run id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    /// Durable run status.
    #[serde(default)]
    pub status: RunStatus,
    /// Parent run identifier when this trace belongs to a delegated child run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_run_id: Option<RunId>,
    /// Parent-scoped delegated task identifier when this trace belongs to a lightweight task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_task_id: Option<TaskId>,
    /// Checkpoint ids in insertion order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checkpoints: Vec<CheckpointId>,
    /// Approval count.
    pub approvals: usize,
    /// Deferred tool count.
    pub deferred_tools: usize,
    /// Latest checkpoint id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_checkpoint: Option<CheckpointId>,
    /// Latest persisted stream cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_cursor: Option<usize>,
    /// Stream cursor references.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stream_cursors: Vec<StreamCursorRef>,
    /// Output preview.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_preview: Option<String>,
    /// Trace context.
    #[serde(default, skip_serializing_if = "TraceContext::is_empty")]
    pub trace_context: TraceContext,
    /// Last update time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
    /// Trace metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

/// Compact session projection for lists, CLI inspect, and UI sidebars.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CompactSessionTrace {
    /// Session id.
    pub session_id: SessionId,
    /// User-facing title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Workspace identifier or path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    /// Profile name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// Durable session status.
    #[serde(default)]
    pub status: SessionStatus,
    /// Number of retained runs.
    pub runs: usize,
    /// Latest run id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_run_id: Option<RunId>,
    /// Last output preview.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_output_preview: Option<String>,
    /// Stream cursor references.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stream_cursors: Vec<StreamCursorRef>,
    /// Trace context.
    #[serde(default, skip_serializing_if = "TraceContext::is_empty")]
    pub trace_context: TraceContext,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Last update time.
    pub updated_at: DateTime<Utc>,
    /// Trace metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}
