//! Workflow, schedule, and heartbeat orchestration contracts.

use std::{collections::BTreeMap, sync::Arc};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::{ClawError, ClawInputPart, ClawResult, WorkspaceBindingSpec};

/// Workflow definition lifecycle status.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowDefinitionStatus {
    /// Draft workflow.
    Draft,
    /// Active workflow.
    #[default]
    Active,
    /// Archived workflow.
    Archived,
}

/// Workflow scope.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowScope {
    /// Global workflow.
    #[default]
    Global,
    /// Session-scoped workflow.
    Session,
}

/// Workflow run status.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowRunStatus {
    /// Created and awaiting execution.
    #[default]
    Queued,
    /// Running.
    Running,
    /// Waiting on node work or external input.
    Waiting,
    /// Completed.
    Completed,
    /// Failed.
    Failed,
    /// Cancelled.
    Cancelled,
}

/// Workflow trigger source.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowTriggerKind {
    /// Web console.
    Web,
    /// API client.
    #[default]
    Api,
    /// Agent tool.
    Agent,
    /// Schedule.
    Schedule,
    /// Bridge.
    Bridge,
    /// System.
    System,
}

/// Schedule status.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleStatus {
    /// Active schedule.
    #[default]
    Active,
    /// Paused schedule.
    Paused,
    /// Completed one-shot schedule.
    Completed,
    /// Deleted schedule.
    Deleted,
}

/// Schedule trigger kind.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleTriggerKind {
    /// Cron expression.
    #[default]
    Cron,
    /// One-shot timestamp.
    Once,
}

/// Schedule execution mode.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleExecutionMode {
    /// Continue a target session.
    ContinueSession,
    /// Fork a source session.
    ForkSession,
    /// Create an isolated session.
    #[default]
    IsolateSession,
    /// Trigger a workflow.
    Workflow,
}

/// Workflow definition create request.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct WorkflowDefinitionCreateRequest {
    /// Workflow name.
    pub name: String,
    /// Optional description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Definition status.
    #[serde(default)]
    pub status: WorkflowDefinitionStatus,
    /// Workflow scope.
    #[serde(default)]
    pub scope: WorkflowScope,
    /// Tags.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Usage guidance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when_to_use: Option<String>,
    /// Argument hint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub argument_hint: Option<String>,
    /// Input schema.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub input_schema: serde_json::Map<String, Value>,
    /// Workflow definition body.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub definition: serde_json::Map<String, Value>,
    /// Metadata.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub metadata: serde_json::Map<String, Value>,
}

/// Workflow trigger request.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct WorkflowTriggerRequest {
    /// Inputs.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub inputs: serde_json::Map<String, Value>,
    /// Profile name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_name: Option<String>,
    /// Workspace binding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<WorkspaceBindingSpec>,
    /// Supervisor session id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supervisor_session_id: Option<String>,
    /// Supervisor run id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supervisor_run_id: Option<String>,
    /// Trigger kind.
    #[serde(default)]
    pub trigger_kind: WorkflowTriggerKind,
    /// Metadata.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub metadata: serde_json::Map<String, Value>,
}

/// Durable workflow definition.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct WorkflowDefinitionRecord {
    /// Workflow id.
    pub id: String,
    /// Workflow name.
    pub name: String,
    /// Optional description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Status.
    pub status: WorkflowDefinitionStatus,
    /// Definition version.
    pub definition_version: u64,
    /// Schema version.
    pub schema_version: String,
    /// Scope.
    pub scope: WorkflowScope,
    /// Tags.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Usage guidance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when_to_use: Option<String>,
    /// Argument hint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub argument_hint: Option<String>,
    /// Input schema.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub input_schema: serde_json::Map<String, Value>,
    /// Definition body.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub definition: serde_json::Map<String, Value>,
    /// Metadata.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub metadata: serde_json::Map<String, Value>,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Update time.
    pub updated_at: DateTime<Utc>,
    /// Archive time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<DateTime<Utc>>,
}

/// Workflow run record.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct WorkflowRunRecord {
    /// Workflow run id.
    pub id: String,
    /// Workflow id.
    pub workflow_id: String,
    /// Workflow version.
    pub workflow_version: u64,
    /// Definition snapshot.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub definition_snapshot: serde_json::Map<String, Value>,
    /// Status.
    pub status: WorkflowRunStatus,
    /// Trigger kind.
    pub trigger_kind: WorkflowTriggerKind,
    /// Supervisor session id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supervisor_session_id: Option<String>,
    /// Supervisor run id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supervisor_run_id: Option<String>,
    /// Profile name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_name: Option<String>,
    /// Workspace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<WorkspaceBindingSpec>,
    /// Inputs.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub inputs: serde_json::Map<String, Value>,
    /// Result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Error message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    /// Current node ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub current_node_ids: Vec<String>,
    /// Metadata.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub metadata: serde_json::Map<String, Value>,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Start time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    /// Finish time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<DateTime<Utc>>,
    /// Update time.
    pub updated_at: DateTime<Utc>,
}

/// Schedule create request.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ScheduleCreateRequest {
    /// Schedule name.
    pub name: String,
    /// Description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Profile name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_name: Option<String>,
    /// Trigger kind.
    #[serde(default)]
    pub trigger_kind: ScheduleTriggerKind,
    /// Cron expression.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cron_expr: Option<String>,
    /// One-shot run time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_at: Option<DateTime<Utc>>,
    /// Timezone.
    #[serde(default = "default_timezone")]
    pub timezone: String,
    /// Execution mode.
    #[serde(default)]
    pub execution_mode: ScheduleExecutionMode,
    /// Target session id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_session_id: Option<String>,
    /// Source session id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_session_id: Option<String>,
    /// Input template.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub input_parts_template: Vec<ClawInputPart>,
    /// Workflow id for workflow mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_id: Option<String>,
    /// Metadata.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub metadata: serde_json::Map<String, Value>,
}

/// Schedule record.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ScheduleRecord {
    /// Schedule id.
    pub id: String,
    /// Name.
    pub name: String,
    /// Description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Status.
    pub status: ScheduleStatus,
    /// Profile name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_name: Option<String>,
    /// Trigger kind.
    pub trigger_kind: ScheduleTriggerKind,
    /// Cron expression.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cron_expr: Option<String>,
    /// One-shot run time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_at: Option<DateTime<Utc>>,
    /// Timezone.
    pub timezone: String,
    /// Next fire time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_fire_at: Option<DateTime<Utc>>,
    /// Execution mode.
    pub execution_mode: ScheduleExecutionMode,
    /// Target session id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_session_id: Option<String>,
    /// Source session id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_session_id: Option<String>,
    /// Input template.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub input_parts_template: Vec<ClawInputPart>,
    /// Workflow id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_id: Option<String>,
    /// Metadata.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub metadata: serde_json::Map<String, Value>,
    /// Last fire time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_fire_at: Option<DateTime<Utc>>,
    /// Fire count.
    pub fire_count: u64,
    /// Failure count.
    pub failure_count: u64,
    /// Creation time.
    pub created_at: DateTime<Utc>,
    /// Update time.
    pub updated_at: DateTime<Utc>,
}

/// Heartbeat status.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct HeartbeatStatus {
    /// Enabled flag.
    pub enabled: bool,
    /// Status string.
    pub status: String,
    /// Last fire time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_fire_at: Option<DateTime<Utc>>,
    /// Last run id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_id: Option<String>,
}

/// In-process orchestration catalog.
#[derive(Clone, Debug, Default)]
pub struct OrchestrationCatalog {
    workflows: Arc<RwLock<BTreeMap<String, WorkflowDefinitionRecord>>>,
    workflow_runs: Arc<RwLock<BTreeMap<String, WorkflowRunRecord>>>,
    schedules: Arc<RwLock<BTreeMap<String, ScheduleRecord>>>,
}

impl OrchestrationCatalog {
    /// Create a workflow definition.
    pub async fn create_workflow(
        &self,
        request: WorkflowDefinitionCreateRequest,
    ) -> ClawResult<WorkflowDefinitionRecord> {
        if request.name.trim().is_empty() {
            return Err(ClawError::InvalidRequest(
                "workflow name is required".to_string(),
            ));
        }
        let now = Utc::now();
        let record = WorkflowDefinitionRecord {
            id: prefixed_id("workflow"),
            name: request.name,
            description: request.description,
            status: request.status,
            definition_version: 1,
            schema_version: "starweaver-claw.workflow.v1".to_string(),
            scope: request.scope,
            tags: request.tags,
            when_to_use: request.when_to_use,
            argument_hint: request.argument_hint,
            input_schema: request.input_schema,
            definition: request.definition,
            metadata: request.metadata,
            created_at: now,
            updated_at: now,
            archived_at: None,
        };
        self.workflows
            .write()
            .await
            .insert(record.id.clone(), record.clone());
        Ok(record)
    }

    /// List workflow definitions.
    pub async fn list_workflows(&self) -> Vec<WorkflowDefinitionRecord> {
        self.workflows.read().await.values().cloned().collect()
    }

    /// Get one workflow definition.
    pub async fn get_workflow(&self, workflow_id: &str) -> ClawResult<WorkflowDefinitionRecord> {
        self.workflows
            .read()
            .await
            .get(workflow_id)
            .cloned()
            .ok_or_else(|| ClawError::NotFound(format!("workflow '{workflow_id}' was not found")))
    }

    /// Update one workflow definition with partial JSON fields.
    pub async fn update_workflow(
        &self,
        workflow_id: &str,
        patch: serde_json::Map<String, Value>,
    ) -> ClawResult<WorkflowDefinitionRecord> {
        let mut workflows = self.workflows.write().await;
        let workflow = workflows.get_mut(workflow_id).ok_or_else(|| {
            ClawError::NotFound(format!("workflow '{workflow_id}' was not found"))
        })?;
        if let Some(name) = patch.get("name").and_then(Value::as_str) {
            workflow.name = name.to_string();
        }
        if let Some(description) = patch.get("description") {
            workflow.description = description.as_str().map(ToOwned::to_owned);
        }
        if let Some(status) = patch.get("status") {
            workflow.status = serde_json::from_value(status.clone())?;
            workflow.archived_at =
                (workflow.status == WorkflowDefinitionStatus::Archived).then(Utc::now);
        }
        if let Some(tags) = patch.get("tags") {
            workflow.tags = serde_json::from_value(tags.clone())?;
        }
        if let Some(when_to_use) = patch.get("when_to_use") {
            workflow.when_to_use = when_to_use.as_str().map(ToOwned::to_owned);
        }
        if let Some(argument_hint) = patch.get("argument_hint") {
            workflow.argument_hint = argument_hint.as_str().map(ToOwned::to_owned);
        }
        if let Some(input_schema) = patch.get("input_schema") {
            workflow.input_schema = serde_json::from_value(input_schema.clone())?;
        }
        if let Some(definition) = patch.get("definition") {
            workflow.definition = serde_json::from_value(definition.clone())?;
        }
        if let Some(metadata) = patch.get("metadata") {
            workflow.metadata = serde_json::from_value(metadata.clone())?;
        }
        workflow.definition_version = workflow.definition_version.saturating_add(1);
        workflow.updated_at = Utc::now();
        Ok(workflow.clone())
    }

    /// Trigger a workflow run.
    pub async fn trigger_workflow(
        &self,
        workflow_id: &str,
        request: WorkflowTriggerRequest,
    ) -> ClawResult<WorkflowRunRecord> {
        let workflow = self
            .workflows
            .read()
            .await
            .get(workflow_id)
            .cloned()
            .ok_or_else(|| {
                ClawError::NotFound(format!("workflow '{workflow_id}' was not found"))
            })?;
        let now = Utc::now();
        let record = WorkflowRunRecord {
            id: prefixed_id("workflow_run"),
            workflow_id: workflow.id,
            workflow_version: workflow.definition_version,
            definition_snapshot: workflow.definition,
            status: WorkflowRunStatus::Queued,
            trigger_kind: request.trigger_kind,
            supervisor_session_id: request.supervisor_session_id,
            supervisor_run_id: request.supervisor_run_id,
            profile_name: request.profile_name,
            workspace: request.workspace,
            inputs: request.inputs,
            result: None,
            error_message: None,
            current_node_ids: Vec::new(),
            metadata: request.metadata,
            created_at: now,
            started_at: None,
            finished_at: None,
            updated_at: now,
        };
        self.workflow_runs
            .write()
            .await
            .insert(record.id.clone(), record.clone());
        Ok(record)
    }

    /// List workflow runs.
    pub async fn list_workflow_runs(&self) -> Vec<WorkflowRunRecord> {
        self.workflow_runs.read().await.values().cloned().collect()
    }

    /// Get one workflow run.
    pub async fn get_workflow_run(&self, run_id: &str) -> ClawResult<WorkflowRunRecord> {
        self.workflow_runs
            .read()
            .await
            .get(run_id)
            .cloned()
            .ok_or_else(|| ClawError::NotFound(format!("workflow run '{run_id}' was not found")))
    }

    /// Create a schedule.
    pub async fn create_schedule(
        &self,
        request: ScheduleCreateRequest,
    ) -> ClawResult<ScheduleRecord> {
        if request.name.trim().is_empty() {
            return Err(ClawError::InvalidRequest(
                "schedule name is required".to_string(),
            ));
        }
        let now = Utc::now();
        let next_fire_at = request.run_at;
        let record = ScheduleRecord {
            id: prefixed_id("schedule"),
            name: request.name,
            description: request.description,
            status: ScheduleStatus::Active,
            profile_name: request.profile_name,
            trigger_kind: request.trigger_kind,
            cron_expr: request.cron_expr,
            run_at: request.run_at,
            timezone: request.timezone,
            next_fire_at,
            execution_mode: request.execution_mode,
            target_session_id: request.target_session_id,
            source_session_id: request.source_session_id,
            input_parts_template: request.input_parts_template,
            workflow_id: request.workflow_id,
            metadata: request.metadata,
            last_fire_at: None,
            fire_count: 0,
            failure_count: 0,
            created_at: now,
            updated_at: now,
        };
        self.schedules
            .write()
            .await
            .insert(record.id.clone(), record.clone());
        Ok(record)
    }

    /// List schedules.
    pub async fn list_schedules(&self) -> Vec<ScheduleRecord> {
        self.schedules.read().await.values().cloned().collect()
    }

    /// Get one schedule.
    pub async fn get_schedule(&self, schedule_id: &str) -> ClawResult<ScheduleRecord> {
        self.schedules
            .read()
            .await
            .get(schedule_id)
            .cloned()
            .ok_or_else(|| ClawError::NotFound(format!("schedule '{schedule_id}' was not found")))
    }

    /// Patch one schedule status or metadata fields.
    pub async fn update_schedule(
        &self,
        schedule_id: &str,
        patch: serde_json::Map<String, Value>,
    ) -> ClawResult<ScheduleRecord> {
        let mut schedules = self.schedules.write().await;
        let schedule = schedules.get_mut(schedule_id).ok_or_else(|| {
            ClawError::NotFound(format!("schedule '{schedule_id}' was not found"))
        })?;
        if let Some(name) = patch.get("name").and_then(Value::as_str) {
            schedule.name = name.to_string();
        }
        if let Some(description) = patch.get("description") {
            schedule.description = description.as_str().map(ToOwned::to_owned);
        }
        if let Some(enabled) = patch.get("enabled").and_then(Value::as_bool) {
            schedule.status = if enabled {
                ScheduleStatus::Active
            } else {
                ScheduleStatus::Paused
            };
        }
        if let Some(cron) = patch.get("cron").and_then(Value::as_str) {
            schedule.cron_expr = Some(cron.to_string());
            schedule.trigger_kind = ScheduleTriggerKind::Cron;
        }
        if let Some(timezone) = patch.get("timezone").and_then(Value::as_str) {
            schedule.timezone = timezone.to_string();
        }
        if let Some(metadata) = patch.get("metadata") {
            schedule.metadata = serde_json::from_value(metadata.clone())?;
        }
        schedule.updated_at = Utc::now();
        Ok(schedule.clone())
    }

    /// Mark one schedule deleted.
    pub async fn delete_schedule(&self, schedule_id: &str) -> ClawResult<ScheduleRecord> {
        let mut schedules = self.schedules.write().await;
        let schedule = schedules.get_mut(schedule_id).ok_or_else(|| {
            ClawError::NotFound(format!("schedule '{schedule_id}' was not found"))
        })?;
        schedule.status = ScheduleStatus::Deleted;
        schedule.updated_at = Utc::now();
        Ok(schedule.clone())
    }
}

fn default_timezone() -> String {
    "UTC".to_string()
}

fn prefixed_id(prefix: &str) -> String {
    format!("{prefix}_{}", Uuid::new_v4())
}
