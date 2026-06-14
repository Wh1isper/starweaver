//! Agent-managed task records and task board snapshots.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use starweaver_core::Metadata;

/// Custom event kind emitted with a full task board snapshot.
pub const TASK_SNAPSHOT_EVENT_KIND: &str = "task_snapshot";

pub const TASK_STATE_DOMAIN: &str = "tasks";

/// Agent-managed task record used by task tools and display snapshots.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Task {
    /// Task identifier.
    pub id: String,
    /// Task title in imperative form.
    pub subject: String,
    /// Detailed task description.
    pub description: String,
    /// Current task status: `pending`, `in_progress`, or `completed`.
    pub status: String,
    /// Present-progress label shown while in progress.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_form: Option<String>,
    /// Optional task owner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// Task ids this task is blocked by.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked_by: Vec<String>,
    /// Task ids this task blocks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocks: Vec<String>,
    /// Additional task metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

impl Task {
    /// Create a pending task.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        subject: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            subject: subject.into(),
            description: description.into(),
            status: "pending".to_string(),
            active_form: None,
            owner: None,
            blocked_by: Vec::new(),
            blocks: Vec::new(),
            metadata: Metadata::default(),
        }
    }
}

/// Full task board snapshot transported through custom events.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct TaskSnapshot {
    /// Full task list in display order.
    pub tasks: Vec<Task>,
}

impl TaskSnapshot {
    /// Return the snapshot as event payload.
    #[must_use]
    pub fn into_payload(self) -> Value {
        serde_json::to_value(self).unwrap_or_else(|_| serde_json::json!({"tasks": []}))
    }
}
