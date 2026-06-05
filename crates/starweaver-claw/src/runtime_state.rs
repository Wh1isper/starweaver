//! In-process runtime state for active single-node execution.

use std::{collections::BTreeMap, sync::Arc};

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

/// In-memory runtime state.
#[derive(Clone, Debug, Default)]
pub struct ClawRuntimeState {
    handles: Arc<RwLock<BTreeMap<String, RuntimeRunHandle>>>,
    session_latest_run_ids: Arc<RwLock<BTreeMap<String, String>>>,
    session_locks: Arc<Mutex<BTreeMap<String, Arc<Mutex<()>>>>>,
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
}
