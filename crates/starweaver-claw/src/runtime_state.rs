//! In-process runtime state for active single-node execution.

use std::{collections::BTreeMap, sync::Arc};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{Mutex, RwLock};

/// Active run handle tracked by the single-node service process.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RuntimeRunHandle {
    /// Run id.
    pub run_id: String,
    /// Session id.
    pub session_id: String,
    /// Dispatch mode.
    pub dispatch_mode: String,
    /// Buffered steering inputs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steering_inputs: Vec<Vec<Value>>,
    /// Requested stop reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub termination_requested: Option<String>,
    /// Closed flag.
    pub closed: bool,
}

impl RuntimeRunHandle {
    /// Build a run handle.
    #[must_use]
    pub fn new(
        run_id: impl Into<String>,
        session_id: impl Into<String>,
        dispatch_mode: impl Into<String>,
    ) -> Self {
        Self {
            run_id: run_id.into(),
            session_id: session_id.into(),
            dispatch_mode: dispatch_mode.into(),
            steering_inputs: Vec::new(),
            termination_requested: None,
            closed: false,
        }
    }
}

/// Runtime async task state.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RuntimeAsyncTask {
    /// Task id.
    pub task_id: String,
    /// Optional task name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Parent session id.
    pub session_id: String,
    /// Task status.
    pub status: String,
    /// Request payload.
    pub payload: Value,
    /// Optional child session id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_session_id: Option<String>,
    /// Optional child run id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_run_id: Option<String>,
    /// Steering payloads.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steering: Vec<Value>,
    /// Created time.
    pub created_at: DateTime<Utc>,
    /// Updated time.
    pub updated_at: DateTime<Utc>,
}

impl RuntimeAsyncTask {
    /// Build an async task record.
    #[must_use]
    pub fn new(session_id: impl Into<String>, task_id: impl Into<String>, payload: Value) -> Self {
        let now = Utc::now();
        let name = payload
            .get("name")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        Self {
            task_id: task_id.into(),
            name,
            session_id: session_id.into(),
            status: "queued".to_string(),
            payload,
            child_session_id: None,
            child_run_id: None,
            steering: Vec::new(),
            created_at: now,
            updated_at: now,
        }
    }
}

/// Runtime session memory state.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RuntimeMemoryState {
    /// Session id.
    pub session_id: String,
    /// Memory action kind.
    pub kind: String,
    /// Action status.
    pub status: String,
    /// Optional generated run id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// Action payload.
    pub payload: Value,
    /// Updated time.
    pub updated_at: DateTime<Utc>,
}

/// Runtime HITL interaction response.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RuntimeInteractionResponse {
    /// Run id.
    pub run_id: String,
    /// Interaction id.
    pub interaction_id: String,
    /// Response payload.
    pub response: Value,
    /// Created time.
    pub created_at: DateTime<Utc>,
}

/// Runtime agency fire state.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RuntimeAgencyFire {
    /// Fire id.
    pub id: String,
    /// Fire status.
    pub status: String,
    /// Source payload.
    pub payload: Value,
    /// Created time.
    pub created_at: DateTime<Utc>,
    /// Updated time.
    pub updated_at: DateTime<Utc>,
}

/// Runtime agency state.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct RuntimeAgencyState {
    /// Enabled flag.
    pub enabled: bool,
    /// Agency state label.
    pub state: String,
    /// Agency session id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agency_session_id: Option<String>,
    /// Latest run id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_run_id: Option<String>,
    /// Active run id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_run_id: Option<String>,
    /// Last fire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_fire: Option<RuntimeAgencyFire>,
}

/// In-memory runtime state.
#[derive(Clone, Debug, Default)]
pub struct ClawRuntimeState {
    handles: Arc<RwLock<BTreeMap<String, RuntimeRunHandle>>>,
    session_latest_run_ids: Arc<RwLock<BTreeMap<String, String>>>,
    session_locks: Arc<Mutex<BTreeMap<String, Arc<Mutex<()>>>>>,
    async_tasks: Arc<RwLock<BTreeMap<String, RuntimeAsyncTask>>>,
    session_async_task_ids: Arc<RwLock<BTreeMap<String, Vec<String>>>>,
    memory_states: Arc<RwLock<BTreeMap<String, RuntimeMemoryState>>>,
    interaction_responses: Arc<RwLock<BTreeMap<String, RuntimeInteractionResponse>>>,
    agency_state: Arc<RwLock<RuntimeAgencyState>>,
    agency_fires: Arc<RwLock<Vec<RuntimeAgencyFire>>>,
}

impl ClawRuntimeState {
    /// Build an empty runtime state.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Return a per-session async lock.
    pub async fn session_lock(&self, session_id: &str) -> Arc<Mutex<()>> {
        let mut locks = self.session_locks.lock().await;
        locks
            .entry(session_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    /// Register an active run handle.
    pub async fn register_run(
        &self,
        session_id: impl Into<String>,
        run_id: impl Into<String>,
        dispatch_mode: impl Into<String>,
    ) -> RuntimeRunHandle {
        let session_id = session_id.into();
        let run_id = run_id.into();
        let handle = RuntimeRunHandle::new(run_id.clone(), session_id.clone(), dispatch_mode);
        self.handles
            .write()
            .await
            .insert(run_id.clone(), handle.clone());
        self.session_latest_run_ids
            .write()
            .await
            .insert(session_id, run_id);
        handle
    }

    /// Load a run handle.
    pub async fn get_run_handle(&self, run_id: &str) -> Option<RuntimeRunHandle> {
        self.handles.read().await.get(run_id).cloned()
    }

    /// Load latest run handle for a session.
    pub async fn get_session_run_handle(&self, session_id: &str) -> Option<RuntimeRunHandle> {
        let run_id = self
            .session_latest_run_ids
            .read()
            .await
            .get(session_id)
            .cloned()?;
        self.get_run_handle(&run_id).await
    }

    /// Record steering input for a running run.
    pub async fn record_steering(
        &self,
        run_id: &str,
        input_parts: Vec<Value>,
    ) -> Option<RuntimeRunHandle> {
        let mut handles = self.handles.write().await;
        let handle = handles.get_mut(run_id)?;
        handle.steering_inputs.push(input_parts);
        Some(handle.clone())
    }

    /// Request run stop.
    pub async fn request_stop(
        &self,
        run_id: &str,
        reason: impl Into<String>,
    ) -> Option<RuntimeRunHandle> {
        let mut handles = self.handles.write().await;
        let handle = handles.get_mut(run_id)?;
        handle.termination_requested = Some(reason.into());
        Some(handle.clone())
    }

    /// Mark a handle closed.
    pub async fn close_run(&self, run_id: &str) {
        let mut handles = self.handles.write().await;
        if let Some(handle) = handles.get_mut(run_id) {
            handle.closed = true;
        }
    }

    /// Clear a run handle.
    pub async fn clear_run(&self, run_id: &str) {
        let handle = self.handles.write().await.remove(run_id);
        if let Some(handle) = handle {
            let mut latest = self.session_latest_run_ids.write().await;
            if latest
                .get(&handle.session_id)
                .is_some_and(|value| value == run_id)
            {
                latest.remove(&handle.session_id);
            }
        }
    }

    /// Snapshot active run handles.
    pub async fn active_runs(&self) -> Vec<RuntimeRunHandle> {
        self.handles.read().await.values().cloned().collect()
    }

    /// Create an async task.
    pub async fn create_async_task(&self, session_id: &str, payload: Value) -> RuntimeAsyncTask {
        let task_id = payload
            .get("task_id")
            .or_else(|| payload.get("id"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| format!("task_{}", Utc::now().timestamp_micros()));
        let task = RuntimeAsyncTask::new(session_id.to_string(), task_id.clone(), payload);
        self.async_tasks
            .write()
            .await
            .insert(task_id.clone(), task.clone());
        self.session_async_task_ids
            .write()
            .await
            .entry(session_id.to_string())
            .or_default()
            .push(task_id);
        task
    }

    /// List async tasks for a session.
    pub async fn list_async_tasks(&self, session_id: &str) -> Vec<RuntimeAsyncTask> {
        let ids = self
            .session_async_task_ids
            .read()
            .await
            .get(session_id)
            .cloned()
            .unwrap_or_default();
        let tasks = self.async_tasks.read().await;
        ids.iter()
            .filter_map(|id| tasks.get(id).cloned())
            .collect::<Vec<_>>()
    }

    /// Get an async task by id or name.
    pub async fn get_async_task(
        &self,
        session_id: &str,
        task_id_or_name: &str,
    ) -> Option<RuntimeAsyncTask> {
        self.list_async_tasks(session_id)
            .await
            .into_iter()
            .find(|task| {
                task.task_id == task_id_or_name
                    || task.name.as_deref() == Some(task_id_or_name)
                    || Some(task_id_or_name)
                        == task
                            .task_id
                            .strip_suffix(":cancel")
                            .or_else(|| task.task_id.strip_suffix(":steer"))
            })
    }

    /// Update async task status.
    pub async fn update_async_task_status(
        &self,
        session_id: &str,
        task_id_or_name: &str,
        status: impl Into<String>,
    ) -> Option<RuntimeAsyncTask> {
        let task = self.get_async_task(session_id, task_id_or_name).await?;
        let mut tasks = self.async_tasks.write().await;
        let task = tasks.get_mut(&task.task_id)?;
        task.status = status.into();
        task.updated_at = Utc::now();
        Some(task.clone())
    }

    /// Record async task steering.
    pub async fn steer_async_task(
        &self,
        session_id: &str,
        task_id_or_name: &str,
        payload: Value,
    ) -> Option<RuntimeAsyncTask> {
        let task = self.get_async_task(session_id, task_id_or_name).await?;
        let mut tasks = self.async_tasks.write().await;
        let task = tasks.get_mut(&task.task_id)?;
        task.status = "running".to_string();
        task.steering.push(payload);
        task.updated_at = Utc::now();
        Some(task.clone())
    }

    /// Upsert session memory state.
    pub async fn upsert_memory_state(
        &self,
        session_id: &str,
        kind: &str,
        payload: Value,
    ) -> RuntimeMemoryState {
        let state = RuntimeMemoryState {
            session_id: session_id.to_string(),
            kind: kind.to_string(),
            status: "queued".to_string(),
            run_id: None,
            payload,
            updated_at: Utc::now(),
        };
        self.memory_states
            .write()
            .await
            .insert(session_id.to_string(), state.clone());
        state
    }

    /// Load session memory state.
    pub async fn memory_state(&self, session_id: &str) -> Option<RuntimeMemoryState> {
        self.memory_states.read().await.get(session_id).cloned()
    }

    /// Record a HITL interaction response.
    pub async fn record_interaction_response(
        &self,
        run_id: &str,
        interaction_id: &str,
        response: Value,
    ) -> RuntimeInteractionResponse {
        let record = RuntimeInteractionResponse {
            run_id: run_id.to_string(),
            interaction_id: interaction_id.to_string(),
            response,
            created_at: Utc::now(),
        };
        self.interaction_responses
            .write()
            .await
            .insert(format!("{run_id}:{interaction_id}"), record.clone());
        record
    }

    /// Snapshot interaction responses.
    pub async fn interaction_responses(&self) -> Vec<RuntimeInteractionResponse> {
        self.interaction_responses
            .read()
            .await
            .values()
            .cloned()
            .collect()
    }

    /// Bootstrap agency runtime state.
    pub async fn bootstrap_agency(&self) -> RuntimeAgencyState {
        let mut state = self.agency_state.write().await;
        state.enabled = true;
        state.state = "idle".to_string();
        state.agency_session_id = Some("agency_single_node".to_string());
        state.clone()
    }

    /// Clear agency runtime state.
    pub async fn clear_agency(&self) -> RuntimeAgencyState {
        let mut state = self.agency_state.write().await;
        *state = RuntimeAgencyState {
            enabled: false,
            state: "idle".to_string(),
            agency_session_id: Some("agency_disabled".to_string()),
            latest_run_id: None,
            active_run_id: None,
            last_fire: None,
        };
        self.agency_fires.write().await.clear();
        state.clone()
    }

    /// Load agency state.
    pub async fn agency_state(&self) -> RuntimeAgencyState {
        let state = self.agency_state.read().await.clone();
        if state.state.is_empty() {
            RuntimeAgencyState {
                enabled: false,
                state: "idle".to_string(),
                agency_session_id: Some("agency_disabled".to_string()),
                latest_run_id: None,
                active_run_id: None,
                last_fire: None,
            }
        } else {
            state
        }
    }

    /// Create an agency fire.
    pub async fn create_agency_fire(&self, payload: Value) -> RuntimeAgencyFire {
        let now = Utc::now();
        let fire = RuntimeAgencyFire {
            id: format!("agency_fire_{}", now.timestamp_micros()),
            status: "submitted".to_string(),
            payload,
            created_at: now,
            updated_at: now,
        };
        self.agency_fires.write().await.push(fire.clone());
        let mut state = self.agency_state.write().await;
        if state.state.is_empty() {
            state.state = "idle".to_string();
        }
        state.last_fire = Some(fire.clone());
        fire
    }

    /// List agency fires.
    pub async fn agency_fires(&self) -> Vec<RuntimeAgencyFire> {
        self.agency_fires.read().await.clone()
    }
}
