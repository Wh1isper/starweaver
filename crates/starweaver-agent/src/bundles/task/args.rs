//! Task tool argument schemas.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub(super) struct TaskCreateArgs {
    /// Task title in imperative form.
    pub(super) subject: String,
    /// Detailed task description.
    pub(super) description: String,
    /// Present progressive form shown during `in_progress`.
    #[serde(default)]
    pub(super) active_form: Option<String>,
    /// Optional additional metadata.
    #[serde(default)]
    pub(super) metadata: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub(super) struct TaskIdArgs {
    /// The task ID to retrieve.
    pub(super) task_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub(super) struct TaskUpdateArgs {
    /// The task ID to update.
    pub(super) task_id: String,
    /// New task status.
    #[serde(default)]
    pub(super) status: Option<String>,
    /// New task title.
    #[serde(default)]
    pub(super) subject: Option<String>,
    /// New task description.
    #[serde(default)]
    pub(super) description: Option<String>,
    /// New present progressive form.
    #[serde(default)]
    pub(super) active_form: Option<String>,
    /// Task owner or assignee.
    #[serde(default)]
    pub(super) owner: Option<String>,
    /// Task IDs that this task blocks.
    #[serde(default)]
    pub(super) add_blocks: Option<Vec<String>>,
    /// Task IDs that block this task.
    #[serde(default)]
    pub(super) add_blocked_by: Option<Vec<String>>,
    /// Metadata to merge into task.
    #[serde(default)]
    pub(super) metadata: Option<Value>,
}
