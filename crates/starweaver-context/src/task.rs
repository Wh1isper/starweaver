//! Agent-managed task records and typed task manager.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use starweaver_core::Metadata;

/// Custom event kind emitted with a full task board snapshot.
pub const TASK_SNAPSHOT_EVENT_KIND: &str = "task_snapshot";

/// Backward-compatible state domain used by older serialized contexts.
pub const TASK_STATE_DOMAIN: &str = "tasks";

/// Task execution status.
#[derive(Clone, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// Task is pending.
    #[default]
    Pending,
    /// Task is currently in progress.
    InProgress,
    /// Task is completed.
    Completed,
}

impl TaskStatus {
    /// Parse a strict ya-mono task status string.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "pending" => Some(Self::Pending),
            "in_progress" => Some(Self::InProgress),
            "completed" => Some(Self::Completed),
            _ => None,
        }
    }

    /// Return normalized string representation.
    #[must_use]
    pub const fn as_str(&self) -> &str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
        }
    }

    /// Return whether this status is completed.
    #[must_use]
    pub const fn is_completed(&self) -> bool {
        matches!(self, Self::Completed)
    }
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

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
    #[serde(default)]
    pub status: TaskStatus,
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
    /// Task creation timestamp.
    #[serde(default = "Utc::now")]
    pub created_at: DateTime<Utc>,
    /// Last update timestamp.
    #[serde(default = "Utc::now")]
    pub updated_at: DateTime<Utc>,
}

impl Task {
    /// Create a pending task.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        subject: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: id.into(),
            subject: subject.into(),
            description: description.into(),
            status: TaskStatus::Pending,
            active_form: None,
            owner: None,
            blocked_by: Vec::new(),
            blocks: Vec::new(),
            metadata: Metadata::default(),
            created_at: now,
            updated_at: now,
        }
    }

    /// Return whether task is blocked by any incomplete tasks.
    #[must_use]
    pub fn is_blocked(&self) -> bool {
        !self.blocked_by.is_empty()
    }

    /// Return status as a string.
    #[must_use]
    pub const fn status_str(&self) -> &str {
        self.status.as_str()
    }

    /// Set status from a parsed status enum.
    pub fn set_status(&mut self, status: TaskStatus) {
        self.status = status;
        self.updated_at = Utc::now();
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

/// Manager for task lifecycle and dependencies.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct TaskManager {
    /// All tasks keyed by task ID.
    #[serde(default)]
    pub tasks: BTreeMap<String, Task>,
}

impl TaskManager {
    /// Create an empty task manager.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a manager from tasks.
    #[must_use]
    pub fn from_tasks(tasks: Vec<Task>) -> Self {
        Self {
            tasks: tasks
                .into_iter()
                .map(|task| (task.id.clone(), task))
                .collect(),
        }
    }

    /// Create a new task.
    pub fn create(
        &mut self,
        subject: impl Into<String>,
        description: impl Into<String>,
        active_form: Option<String>,
        metadata: Metadata,
    ) -> Task {
        let id = self.next_id();
        let mut task = Task::new(id.clone(), subject, description);
        task.active_form = active_form;
        task.metadata = metadata;
        self.tasks.insert(id, task.clone());
        task
    }

    /// Return a task by id.
    #[must_use]
    pub fn get(&self, task_id: &str) -> Option<&Task> {
        self.tasks.get(task_id)
    }

    /// Return all tasks in stable numeric order.
    #[must_use]
    pub fn list_all(&self) -> Vec<Task> {
        let mut tasks = self.tasks.values().cloned().collect::<Vec<_>>();
        tasks.sort_by_key(|task| task_sort_key(&task.id));
        tasks
    }

    /// Replace all tasks from display snapshot order.
    pub fn replace_all(&mut self, tasks: Vec<Task>) {
        self.tasks = tasks
            .into_iter()
            .map(|task| (normalize_task_id(&task.id), task))
            .collect();
    }

    /// Update one task and resolve dependencies when completed.
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        task_id: &str,
        status: Option<TaskStatus>,
        subject: Option<String>,
        description: Option<String>,
        active_form: Option<Option<String>>,
        owner: Option<Option<String>>,
        add_blocks: Option<&[String]>,
        add_blocked_by: Option<&[String]>,
        metadata: Option<&Metadata>,
    ) -> Option<Task> {
        let task_id = normalize_task_id(task_id);
        let was_completed = status.as_ref().is_some_and(TaskStatus::is_completed)
            && self
                .tasks
                .get(&task_id)
                .is_some_and(|task| !task.status.is_completed());
        {
            let task = self.tasks.get_mut(&task_id)?;
            if let Some(status) = status {
                task.status = status;
            }
            if let Some(subject) = subject.filter(|value| !value.trim().is_empty()) {
                task.subject = subject;
            }
            if let Some(description) = description.filter(|value| !value.trim().is_empty()) {
                task.description = description;
            }
            if let Some(active_form) = active_form {
                task.active_form = active_form;
            }
            if let Some(owner) = owner {
                task.owner = owner;
            }
            extend_unique(&mut task.blocks, add_blocks);
            extend_unique(&mut task.blocked_by, add_blocked_by);
            if let Some(metadata) = metadata {
                for (key, value) in metadata {
                    task.metadata.insert(key.clone(), value.clone());
                }
            }
            task.updated_at = Utc::now();
        }

        for blocked_id in add_blocks.unwrap_or_default() {
            self.add_blocking_relationship(&task_id, blocked_id);
        }
        for blocker_id in add_blocked_by.unwrap_or_default() {
            self.add_blocked_by_relationship(&task_id, blocker_id);
        }
        if was_completed {
            self.resolve_completion(&task_id);
        }
        self.tasks.get(&task_id).cloned()
    }

    /// Export task data keyed by task id.
    #[must_use]
    pub fn export_tasks(&self) -> BTreeMap<String, Value> {
        self.tasks
            .iter()
            .map(|(id, task)| {
                (
                    id.clone(),
                    serde_json::to_value(task).unwrap_or_else(|_| serde_json::json!({})),
                )
            })
            .collect()
    }

    /// Restore from exported task data.
    #[must_use]
    pub fn from_exported(data: BTreeMap<String, Value>) -> Self {
        let tasks = data
            .into_iter()
            .filter_map(|(id, value)| {
                serde_json::from_value::<Task>(value)
                    .ok()
                    .map(|task| (id, task))
            })
            .collect();
        Self { tasks }
    }

    /// Return whether the manager has no tasks.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    fn next_id(&self) -> String {
        self.tasks
            .keys()
            .filter_map(|id| normalize_task_id(id).parse::<u64>().ok())
            .max()
            .unwrap_or(0)
            .saturating_add(1)
            .to_string()
    }

    fn add_blocking_relationship(&mut self, task_id: &str, blocked_id: &str) {
        let blocked_id = normalize_task_id(blocked_id);
        if let Some(task) = self.tasks.get_mut(task_id) {
            extend_unique(&mut task.blocks, Some(std::slice::from_ref(&blocked_id)));
        }
        if let Some(blocked_task) = self.tasks.get_mut(&blocked_id) {
            extend_unique(
                &mut blocked_task.blocked_by,
                Some(std::slice::from_ref(&task_id.to_string())),
            );
            blocked_task.updated_at = Utc::now();
        }
    }

    fn add_blocked_by_relationship(&mut self, task_id: &str, blocker_id: &str) {
        let blocker_id = normalize_task_id(blocker_id);
        if let Some(task) = self.tasks.get_mut(task_id) {
            extend_unique(
                &mut task.blocked_by,
                Some(std::slice::from_ref(&blocker_id)),
            );
        }
        if let Some(blocker) = self.tasks.get_mut(&blocker_id) {
            extend_unique(
                &mut blocker.blocks,
                Some(std::slice::from_ref(&task_id.to_string())),
            );
            blocker.updated_at = Utc::now();
        }
    }

    fn resolve_completion(&mut self, task_id: &str) {
        let Some(blocked_ids) = self.tasks.get(task_id).map(|task| task.blocks.clone()) else {
            return;
        };
        for blocked_id in blocked_ids {
            if let Some(blocked_task) = self.tasks.get_mut(&blocked_id) {
                blocked_task.blocked_by.retain(|blocker| blocker != task_id);
                blocked_task.updated_at = Utc::now();
            }
        }
    }
}

fn normalize_task_id(id: &str) -> String {
    id.trim().trim_start_matches('#').to_string()
}

fn task_sort_key(id: &str) -> (u8, u64, String) {
    normalize_task_id(id).parse::<u64>().map_or_else(
        |_| (1, 0, id.to_string()),
        |value| (0, value, String::new()),
    )
}

fn extend_unique(target: &mut Vec<String>, values: Option<&[String]>) {
    let Some(values) = values else {
        return;
    };
    let mut seen = target
        .iter()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    for value in values {
        let normalized = normalize_task_id(value);
        if !normalized.is_empty() && seen.insert(normalized.clone()) {
            target.push(normalized);
        }
    }
}
