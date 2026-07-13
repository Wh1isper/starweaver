//! Python bindings for native Starweaver session stores.

use std::{path::PathBuf, sync::Arc, time::Duration};

use async_trait::async_trait;
use pyo3::{exceptions::PyValueError, prelude::*};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use starweaver_context::ResumableState;
use starweaver_core::{RunId, SessionId};
use starweaver_runtime::{AgentCheckpoint, AgentStreamRecord};
use starweaver_session::{
    ApprovalRecord, CompactRunTrace, CompactSessionTrace, DeferredToolRecord, EnvironmentStateRef,
    HitlResumeClaim, PendingStreamPublication, RunEvidenceCommit, RunRecord, RunStatus,
    SessionFilter, SessionRecord, SessionResumeSnapshot, SessionStatus, SessionStore,
    SessionStoreError, SessionStoreResult, StreamCursorRef, StreamPublicationTarget,
};
use starweaver_storage::{
    SqliteMigrationStatus, SqliteReplayEventLog, SqliteSessionStore, SqliteStreamArchive,
    migrate_sqlite_database, sqlite_migration_status,
};
use starweaver_stream::{
    DisplayMessage, ReplayCursor, ReplayError, ReplayEvent, ReplayEventLog, ReplayScope,
    ReplaySnapshot, StreamArchive,
};

use crate::{
    conversion::{py_to_json, serialize_to_py},
    runtime::{PyFutureError, spawn_py_future},
};

/// Python-visible SQLite-backed session store handle.
#[pyclass(name = "SqliteSessionStore", skip_from_py_object)]
#[derive(Clone, Debug)]
pub struct PySqliteSessionStore {
    inner: SqliteSessionStore,
}

/// Python-visible SQLite-backed replay event log handle.
#[pyclass(name = "SqliteReplayEventLog", skip_from_py_object)]
#[derive(Clone, Debug)]
pub struct PySqliteReplayEventLog {
    inner: SqliteReplayEventLog,
}

/// Python-visible SQLite-backed stream archive handle.
#[pyclass(name = "SqliteStreamArchive", skip_from_py_object)]
#[derive(Clone, Debug)]
pub struct PySqliteStreamArchive {
    inner: SqliteStreamArchive,
}

/// Python-visible callback-backed session store handle.
#[pyclass(name = "PythonSessionStore", skip_from_py_object)]
#[derive(Clone)]
pub struct PyPythonSessionStore {
    inner: Arc<PythonSessionStore>,
}

struct PythonSessionStore {
    store: Py<PyAny>,
    event_loop: Py<PyAny>,
}

unsafe impl Send for PythonSessionStore {}
unsafe impl Sync for PythonSessionStore {}

impl PythonSessionStore {
    async fn call_method<F>(&self, operation: &str, call: F) -> SessionStoreResult<Py<PyAny>>
    where
        F: for<'py> FnOnce(Python<'py>, &Py<PyAny>) -> PyResult<Py<PyAny>> + Send,
    {
        enum CallbackValue {
            Immediate(Py<PyAny>),
            Future(Py<PyAny>),
        }

        let call_result = Python::attach(|py| -> PyResult<CallbackValue> {
            let value = call(py, &self.store)?;
            let inspect = py.import("inspect")?;
            let is_awaitable = inspect
                .call_method1("isawaitable", (value.bind(py),))?
                .extract::<bool>()?;
            if !is_awaitable {
                return Ok(CallbackValue::Immediate(value));
            }
            let asyncio = py.import("asyncio")?;
            let future = asyncio.call_method1(
                "run_coroutine_threadsafe",
                (value, self.event_loop.clone_ref(py)),
            )?;
            Ok(CallbackValue::Future(future.unbind()))
        })
        .map_err(|error| py_err_to_store_error(operation, error))?;

        let future = match call_result {
            CallbackValue::Immediate(value) => return Ok(value),
            CallbackValue::Future(future) => future,
        };
        let guard_future = Python::attach(|py| future.clone_ref(py));
        let mut cancel_guard = PythonFutureCancelGuard::new(guard_future);
        let mut tick = tokio::time::interval(Duration::from_millis(10));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let result = loop {
            tick.tick().await;
            let poll = Python::attach(|py| -> PyResult<Option<Py<PyAny>>> {
                let done = future.call_method0(py, "done")?.extract::<bool>(py)?;
                if done {
                    Ok(Some(future.call_method0(py, "result")?))
                } else {
                    Ok(None)
                }
            });
            match poll {
                Ok(Some(value)) => break Ok(value),
                Ok(None) => {}
                Err(error) => break Err(error),
            }
        }
        .map_err(|error| py_err_to_store_error(operation, error));
        cancel_guard.complete();
        result
    }

    async fn call_unit<F>(&self, operation: &str, call: F) -> SessionStoreResult<()>
    where
        F: for<'py> FnOnce(Python<'py>, &Py<PyAny>) -> PyResult<Py<PyAny>> + Send,
    {
        self.call_method(operation, call).await.map(|_| ())
    }

    async fn call_json<T, F>(&self, operation: &str, call: F) -> SessionStoreResult<T>
    where
        T: DeserializeOwned,
        F: for<'py> FnOnce(Python<'py>, &Py<PyAny>) -> PyResult<Py<PyAny>> + Send,
    {
        let value = self.call_method(operation, call).await?;
        Python::attach(|py| -> PyResult<T> {
            let normalized = py
                .import("starweaver.store")?
                .getattr("_jsonify")?
                .call1((value.bind(py),))?;
            let value = py_to_json(py, &normalized)?;
            serde_json::from_value::<T>(value).map_err(|error| {
                PyValueError::new_err(format!("invalid {operation} result: {error}"))
            })
        })
        .map_err(|error| py_err_to_store_error(operation, error))
    }
}

struct PythonFutureCancelGuard {
    future: Py<PyAny>,
    completed: bool,
}

impl PythonFutureCancelGuard {
    fn new(future: Py<PyAny>) -> Self {
        Self {
            future,
            completed: false,
        }
    }

    const fn complete(&mut self) {
        self.completed = true;
    }
}

impl Drop for PythonFutureCancelGuard {
    fn drop(&mut self) {
        if self.completed {
            return;
        }
        Python::attach(|py| {
            let _ = self.future.call_method0(py, "cancel");
        });
    }
}

impl PyPythonSessionStore {
    pub(crate) fn session_store(&self) -> Arc<dyn SessionStore> {
        let store: Arc<dyn SessionStore> = self.inner.clone();
        store
    }
}

#[pymethods]
impl PyPythonSessionStore {
    #[new]
    fn new(store: Py<PyAny>, event_loop: Py<PyAny>) -> Self {
        Self {
            inner: Arc::new(PythonSessionStore { store, event_loop }),
        }
    }

    fn save_session(&self, py: Python<'_>, record: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        let record = parse_record::<SessionRecord>(py, record, "session record")?;
        let store = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                store
                    .save_session(record)
                    .await
                    .map_err(store_error_to_future)
            },
            |py, ()| Ok(py.None()),
        )
    }

    fn load_session(&self, py: Python<'_>, session_id: String) -> PyResult<Py<PyAny>> {
        let session_id = SessionId::from_string(session_id);
        let store = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                store
                    .load_session(&session_id)
                    .await
                    .map_err(store_error_to_future)
            },
            |py, record| serialize_to_py(py, &record),
        )
    }

    #[pyo3(signature = (filter=None))]
    fn list_sessions(
        &self,
        py: Python<'_>,
        filter: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        let filter = parse_session_filter(py, filter)?;
        let store = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                store
                    .list_sessions(filter)
                    .await
                    .map_err(store_error_to_future)
            },
            |py, records| serialize_to_py(py, &records),
        )
    }

    fn append_run(&self, py: Python<'_>, record: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        let record = parse_record::<RunRecord>(py, record, "run record")?;
        let store = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                store
                    .append_run(record)
                    .await
                    .map_err(store_error_to_future)
            },
            |py, ()| Ok(py.None()),
        )
    }

    fn update_session_status(
        &self,
        py: Python<'_>,
        session_id: String,
        status: String,
    ) -> PyResult<Py<PyAny>> {
        let session_id = SessionId::from_string(session_id);
        let status = parse_json_value::<SessionStatus>(json!(status), "session status")?;
        let store = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                store
                    .update_session_status(&session_id, status)
                    .await
                    .map_err(store_error_to_future)?;
                Ok(())
            },
            |py, ()| Ok(py.None()),
        )
    }

    fn save_context_state(
        &self,
        py: Python<'_>,
        session_id: String,
        state: &Bound<'_, PyAny>,
    ) -> PyResult<Py<PyAny>> {
        let session_id = SessionId::from_string(session_id);
        let state = parse_record(py, state, "context state")?;
        let store = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                store
                    .save_context_state(&session_id, state)
                    .await
                    .map_err(store_error_to_future)?;
                Ok(())
            },
            |py, ()| Ok(py.None()),
        )
    }

    fn save_environment_state(
        &self,
        py: Python<'_>,
        session_id: String,
        environment_state: &Bound<'_, PyAny>,
    ) -> PyResult<Py<PyAny>> {
        let session_id = SessionId::from_string(session_id);
        let environment_state =
            parse_record::<EnvironmentStateRef>(py, environment_state, "environment state")?;
        let store = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                store
                    .save_environment_state(&session_id, environment_state)
                    .await
                    .map_err(store_error_to_future)?;
                Ok(())
            },
            |py, ()| Ok(py.None()),
        )
    }

    fn load_run(&self, py: Python<'_>, session_id: String, run_id: String) -> PyResult<Py<PyAny>> {
        let session_id = SessionId::from_string(session_id);
        let run_id = RunId::from_string(run_id);
        let store = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                store
                    .load_run(&session_id, &run_id)
                    .await
                    .map_err(store_error_to_future)
            },
            |py, record| serialize_to_py(py, &record),
        )
    }

    fn list_runs(&self, py: Python<'_>, session_id: String) -> PyResult<Py<PyAny>> {
        let session_id = SessionId::from_string(session_id);
        let store = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                store
                    .list_runs(&session_id)
                    .await
                    .map_err(store_error_to_future)
            },
            |py, records| serialize_to_py(py, &records),
        )
    }

    #[pyo3(signature = (session_id, run_id, status, output_preview=None))]
    fn update_run_status(
        &self,
        py: Python<'_>,
        session_id: String,
        run_id: String,
        status: String,
        output_preview: Option<String>,
    ) -> PyResult<Py<PyAny>> {
        let session_id = SessionId::from_string(session_id);
        let run_id = RunId::from_string(run_id);
        let status = parse_json_value::<RunStatus>(json!(status), "run status")?;
        let store = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                store
                    .update_run_status(&session_id, &run_id, status, output_preview)
                    .await
                    .map_err(store_error_to_future)?;
                Ok(())
            },
            |py, ()| Ok(py.None()),
        )
    }

    fn append_checkpoint(
        &self,
        py: Python<'_>,
        session_id: String,
        checkpoint: &Bound<'_, PyAny>,
    ) -> PyResult<Py<PyAny>> {
        let session_id = SessionId::from_string(session_id);
        let checkpoint = parse_record::<AgentCheckpoint>(py, checkpoint, "checkpoint")?;
        let store = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                store
                    .append_checkpoint(&session_id, checkpoint)
                    .await
                    .map_err(store_error_to_future)?;
                Ok(())
            },
            |py, ()| Ok(py.None()),
        )
    }

    fn load_checkpoints(
        &self,
        py: Python<'_>,
        session_id: String,
        run_id: String,
    ) -> PyResult<Py<PyAny>> {
        let session_id = SessionId::from_string(session_id);
        let run_id = RunId::from_string(run_id);
        let store = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                store
                    .load_checkpoints(&session_id, &run_id)
                    .await
                    .map_err(store_error_to_future)
            },
            |py, records| serialize_to_py(py, &records),
        )
    }

    fn append_stream_records(
        &self,
        py: Python<'_>,
        session_id: String,
        run_id: String,
        records: &Bound<'_, PyAny>,
    ) -> PyResult<Py<PyAny>> {
        let session_id = SessionId::from_string(session_id);
        let run_id = RunId::from_string(run_id);
        let records = parse_record::<Vec<AgentStreamRecord>>(py, records, "stream records")?;
        let store = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                store
                    .append_stream_records(&session_id, &run_id, records)
                    .await
                    .map_err(store_error_to_future)?;
                Ok(())
            },
            |py, ()| Ok(py.None()),
        )
    }

    #[pyo3(signature = (session_id, run_id, after_sequence=None))]
    fn replay_stream_records(
        &self,
        py: Python<'_>,
        session_id: String,
        run_id: String,
        after_sequence: Option<usize>,
    ) -> PyResult<Py<PyAny>> {
        let session_id = SessionId::from_string(session_id);
        let run_id = RunId::from_string(run_id);
        let store = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                store
                    .replay_stream_records_after(&session_id, &run_id, after_sequence)
                    .await
                    .map_err(store_error_to_future)
            },
            |py, records| serialize_to_py(py, &records),
        )
    }

    fn save_stream_cursor(
        &self,
        py: Python<'_>,
        session_id: String,
        run_id: String,
        cursor: &Bound<'_, PyAny>,
    ) -> PyResult<Py<PyAny>> {
        let session_id = SessionId::from_string(session_id);
        let run_id = RunId::from_string(run_id);
        let cursor = parse_record::<StreamCursorRef>(py, cursor, "stream cursor")?;
        let store = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                store
                    .save_stream_cursor(&session_id, &run_id, cursor)
                    .await
                    .map_err(store_error_to_future)?;
                Ok(())
            },
            |py, ()| Ok(py.None()),
        )
    }

    fn append_approval(&self, py: Python<'_>, record: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        let record = parse_record::<ApprovalRecord>(py, record, "approval record")?;
        let store = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                store
                    .append_approval(record)
                    .await
                    .map_err(store_error_to_future)?;
                Ok(())
            },
            |py, ()| Ok(py.None()),
        )
    }

    fn load_approvals(
        &self,
        py: Python<'_>,
        session_id: String,
        run_id: String,
    ) -> PyResult<Py<PyAny>> {
        let session_id = SessionId::from_string(session_id);
        let run_id = RunId::from_string(run_id);
        let store = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                store
                    .load_approvals(&session_id, &run_id)
                    .await
                    .map_err(store_error_to_future)
            },
            |py, records| serialize_to_py(py, &records),
        )
    }

    fn append_deferred_tool(
        &self,
        py: Python<'_>,
        record: &Bound<'_, PyAny>,
    ) -> PyResult<Py<PyAny>> {
        let record = parse_record::<DeferredToolRecord>(py, record, "deferred tool record")?;
        let store = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                store
                    .append_deferred_tool(record)
                    .await
                    .map_err(store_error_to_future)?;
                Ok(())
            },
            |py, ()| Ok(py.None()),
        )
    }

    fn load_deferred_tools(
        &self,
        py: Python<'_>,
        session_id: String,
        run_id: String,
    ) -> PyResult<Py<PyAny>> {
        let session_id = SessionId::from_string(session_id);
        let run_id = RunId::from_string(run_id);
        let store = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                store
                    .load_deferred_tools(&session_id, &run_id)
                    .await
                    .map_err(store_error_to_future)
            },
            |py, records| serialize_to_py(py, &records),
        )
    }

    fn resume_snapshot(
        &self,
        py: Python<'_>,
        session_id: String,
        run_id: String,
    ) -> PyResult<Py<PyAny>> {
        let session_id = SessionId::from_string(session_id);
        let run_id = RunId::from_string(run_id);
        let store = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                store
                    .resume_snapshot(&session_id, &run_id)
                    .await
                    .map_err(store_error_to_future)
            },
            |py, snapshot| serialize_to_py(py, &snapshot),
        )
    }

    fn compact_run_trace(
        &self,
        py: Python<'_>,
        session_id: String,
        run_id: String,
    ) -> PyResult<Py<PyAny>> {
        let session_id = SessionId::from_string(session_id);
        let run_id = RunId::from_string(run_id);
        let store = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                store
                    .compact_run_trace(&session_id, &run_id)
                    .await
                    .map_err(store_error_to_future)
            },
            |py, trace| serialize_to_py(py, &trace),
        )
    }

    fn compact_session_trace(&self, py: Python<'_>, session_id: String) -> PyResult<Py<PyAny>> {
        let session_id = SessionId::from_string(session_id);
        let store = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                store
                    .compact_session_trace(&session_id)
                    .await
                    .map_err(store_error_to_future)
            },
            |py, trace| serialize_to_py(py, &trace),
        )
    }
}

#[async_trait]
impl SessionStore for PythonSessionStore {
    async fn commit_run_evidence(
        &self,
        commit: RunEvidenceCommit,
    ) -> SessionStoreResult<RunRecord> {
        self.call_json("commit_run_evidence", move |py, store| {
            let commit = serialize_to_py(py, &commit)?;
            store.call_method1(py, "commit_run_evidence", (commit,))
        })
        .await
    }

    async fn claim_hitl_resume(&self, claim: HitlResumeClaim) -> SessionStoreResult<()> {
        self.call_unit("claim_hitl_resume", move |py, store| {
            let claim = serialize_to_py(py, &claim)?;
            store.call_method1(py, "claim_hitl_resume", (claim,))
        })
        .await
    }

    async fn mark_hitl_resume_started(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        claim_id: &str,
    ) -> SessionStoreResult<()> {
        let session_id = session_id.as_str().to_string();
        let run_id = run_id.as_str().to_string();
        let claim_id = claim_id.to_string();
        self.call_unit("mark_hitl_resume_started", move |py, store| {
            store.call_method1(
                py,
                "mark_hitl_resume_started",
                (session_id, run_id, claim_id),
            )
        })
        .await
    }

    async fn release_hitl_resume_claim(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        claim_id: &str,
    ) -> SessionStoreResult<()> {
        let session_id = session_id.as_str().to_string();
        let run_id = run_id.as_str().to_string();
        let claim_id = claim_id.to_string();
        self.call_unit("release_hitl_resume_claim", move |py, store| {
            store.call_method1(
                py,
                "release_hitl_resume_claim",
                (session_id, run_id, claim_id),
            )
        })
        .await
    }

    async fn pending_stream_publications(
        &self,
        session_id: &SessionId,
    ) -> SessionStoreResult<Vec<PendingStreamPublication>> {
        let session_id = session_id.as_str().to_string();
        self.call_json("pending_stream_publications", move |py, store| {
            store.call_method1(py, "pending_stream_publications", (session_id,))
        })
        .await
    }

    async fn acknowledge_stream_publication(
        &self,
        publication_id: &str,
        target: StreamPublicationTarget,
    ) -> SessionStoreResult<()> {
        let publication_id = publication_id.to_string();
        self.call_unit("acknowledge_stream_publication", move |py, store| {
            let target = serialize_to_py(py, &target)?;
            store.call_method1(
                py,
                "acknowledge_stream_publication",
                (publication_id, target),
            )
        })
        .await
    }

    async fn commit_checkpoint(
        &self,
        session_id: &SessionId,
        checkpoint: AgentCheckpoint,
    ) -> SessionStoreResult<()> {
        let session_id = session_id.as_str().to_string();
        self.call_unit("commit_checkpoint", move |py, store| {
            let checkpoint = serialize_to_py(py, &checkpoint)?;
            store.call_method1(py, "commit_checkpoint", (session_id, checkpoint))
        })
        .await
    }

    async fn save_session(&self, session: SessionRecord) -> SessionStoreResult<()> {
        self.call_unit("save_session", move |py, store| {
            let session = serialize_to_py(py, &session)?;
            store.call_method1(py, "save_session", (session,))
        })
        .await
    }

    async fn load_session(&self, session_id: &SessionId) -> SessionStoreResult<SessionRecord> {
        let session_id = session_id.as_str().to_string();
        self.call_json("load_session", move |py, store| {
            store.call_method1(py, "load_session", (session_id,))
        })
        .await
    }

    async fn list_sessions(&self, filter: SessionFilter) -> SessionStoreResult<Vec<SessionRecord>> {
        self.call_json("list_sessions", move |py, store| {
            let filter = serialize_to_py(py, &session_filter_json(filter))?;
            store.call_method1(py, "list_sessions", (filter,))
        })
        .await
    }

    async fn update_session_status(
        &self,
        session_id: &SessionId,
        status: SessionStatus,
    ) -> SessionStoreResult<()> {
        let session_id = session_id.as_str().to_string();
        self.call_unit("update_session_status", move |py, store| {
            let status = serialize_to_py(py, &status)?;
            store.call_method1(py, "update_session_status", (session_id, status))
        })
        .await
    }

    async fn save_context_state(
        &self,
        session_id: &SessionId,
        state: ResumableState,
    ) -> SessionStoreResult<()> {
        let session_id = session_id.as_str().to_string();
        self.call_unit("save_context_state", move |py, store| {
            let state = serialize_to_py(py, &state)?;
            store.call_method1(py, "save_context_state", (session_id, state))
        })
        .await
    }

    async fn save_environment_state(
        &self,
        session_id: &SessionId,
        environment_state: EnvironmentStateRef,
    ) -> SessionStoreResult<()> {
        let session_id = session_id.as_str().to_string();
        self.call_unit("save_environment_state", move |py, store| {
            let environment_state = serialize_to_py(py, &environment_state)?;
            store.call_method1(
                py,
                "save_environment_state",
                (session_id, environment_state),
            )
        })
        .await
    }

    async fn append_run(&self, run: RunRecord) -> SessionStoreResult<()> {
        self.call_unit("append_run", move |py, store| {
            let run = serialize_to_py(py, &run)?;
            store.call_method1(py, "append_run", (run,))
        })
        .await
    }

    async fn load_run(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<RunRecord> {
        let session_id = session_id.as_str().to_string();
        let run_id = run_id.as_str().to_string();
        self.call_json("load_run", move |py, store| {
            store.call_method1(py, "load_run", (session_id, run_id))
        })
        .await
    }

    async fn list_runs(&self, session_id: &SessionId) -> SessionStoreResult<Vec<RunRecord>> {
        let session_id = session_id.as_str().to_string();
        self.call_json("list_runs", move |py, store| {
            store.call_method1(py, "list_runs", (session_id,))
        })
        .await
    }

    async fn update_run_status(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        status: RunStatus,
        output_preview: Option<String>,
    ) -> SessionStoreResult<()> {
        let session_id = session_id.as_str().to_string();
        let run_id = run_id.as_str().to_string();
        self.call_unit("update_run_status", move |py, store| {
            let status = serialize_to_py(py, &status)?;
            store.call_method1(
                py,
                "update_run_status",
                (session_id, run_id, status, output_preview),
            )
        })
        .await
    }

    async fn append_checkpoint(
        &self,
        session_id: &SessionId,
        checkpoint: AgentCheckpoint,
    ) -> SessionStoreResult<()> {
        let session_id = session_id.as_str().to_string();
        self.call_unit("append_checkpoint", move |py, store| {
            let checkpoint = serialize_to_py(py, &checkpoint)?;
            store.call_method1(py, "append_checkpoint", (session_id, checkpoint))
        })
        .await
    }

    async fn load_checkpoints(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<Vec<AgentCheckpoint>> {
        let session_id = session_id.as_str().to_string();
        let run_id = run_id.as_str().to_string();
        self.call_json("load_checkpoints", move |py, store| {
            store.call_method1(py, "load_checkpoints", (session_id, run_id))
        })
        .await
    }

    async fn append_stream_records(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        records: Vec<AgentStreamRecord>,
    ) -> SessionStoreResult<()> {
        let session_id = session_id.as_str().to_string();
        let run_id = run_id.as_str().to_string();
        self.call_unit("append_stream_records", move |py, store| {
            let records = serialize_to_py(py, &records)?;
            store.call_method1(py, "append_stream_records", (session_id, run_id, records))
        })
        .await
    }

    async fn replay_stream_records(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<Vec<AgentStreamRecord>> {
        let session_id = session_id.as_str().to_string();
        let run_id = run_id.as_str().to_string();
        self.call_json("replay_stream_records", move |py, store| {
            store.call_method1(py, "replay_stream_records", (session_id, run_id))
        })
        .await
    }

    async fn replay_stream_records_after(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        after_sequence: Option<usize>,
    ) -> SessionStoreResult<Vec<AgentStreamRecord>> {
        let session_id = session_id.as_str().to_string();
        let run_id = run_id.as_str().to_string();
        self.call_json("replay_stream_records", move |py, store| {
            store.call_method1(
                py,
                "replay_stream_records",
                (session_id, run_id, after_sequence),
            )
        })
        .await
    }

    async fn save_stream_cursor(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        cursor: StreamCursorRef,
    ) -> SessionStoreResult<()> {
        let session_id = session_id.as_str().to_string();
        let run_id = run_id.as_str().to_string();
        self.call_unit("save_stream_cursor", move |py, store| {
            let cursor = serialize_to_py(py, &cursor)?;
            store.call_method1(py, "save_stream_cursor", (session_id, run_id, cursor))
        })
        .await
    }

    async fn append_approval(&self, approval: ApprovalRecord) -> SessionStoreResult<()> {
        self.call_unit("append_approval", move |py, store| {
            let approval = serialize_to_py(py, &approval)?;
            store.call_method1(py, "append_approval", (approval,))
        })
        .await
    }

    async fn load_approvals(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<Vec<ApprovalRecord>> {
        let session_id = session_id.as_str().to_string();
        let run_id = run_id.as_str().to_string();
        self.call_json("load_approvals", move |py, store| {
            store.call_method1(py, "load_approvals", (session_id, run_id))
        })
        .await
    }

    async fn append_deferred_tool(&self, record: DeferredToolRecord) -> SessionStoreResult<()> {
        self.call_unit("append_deferred_tool", move |py, store| {
            let record = serialize_to_py(py, &record)?;
            store.call_method1(py, "append_deferred_tool", (record,))
        })
        .await
    }

    async fn load_deferred_tools(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<Vec<DeferredToolRecord>> {
        let session_id = session_id.as_str().to_string();
        let run_id = run_id.as_str().to_string();
        self.call_json("load_deferred_tools", move |py, store| {
            store.call_method1(py, "load_deferred_tools", (session_id, run_id))
        })
        .await
    }

    async fn resume_snapshot(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<SessionResumeSnapshot> {
        let session_id = session_id.as_str().to_string();
        let run_id = run_id.as_str().to_string();
        self.call_json("resume_snapshot", move |py, store| {
            store.call_method1(py, "resume_snapshot", (session_id, run_id))
        })
        .await
    }

    async fn compact_run_trace(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<CompactRunTrace> {
        let session_id = session_id.as_str().to_string();
        let run_id = run_id.as_str().to_string();
        self.call_json("compact_run_trace", move |py, store| {
            store.call_method1(py, "compact_run_trace", (session_id, run_id))
        })
        .await
    }

    async fn compact_session_trace(
        &self,
        session_id: &SessionId,
    ) -> SessionStoreResult<CompactSessionTrace> {
        let session_id = session_id.as_str().to_string();
        self.call_json("compact_session_trace", move |py, store| {
            store.call_method1(py, "compact_session_trace", (session_id,))
        })
        .await
    }
}

impl PySqliteSessionStore {
    pub(crate) fn session_store(&self) -> Arc<dyn SessionStore> {
        Arc::new(self.inner.clone())
    }
}

#[pymethods]
impl PySqliteSessionStore {
    #[new]
    fn new(path: String) -> PyResult<Self> {
        Self::open(path)
    }

    /// Open or create a SQLite store and apply native migrations.
    #[staticmethod]
    fn open(path: String) -> PyResult<Self> {
        Ok(Self {
            inner: SqliteSessionStore::open(PathBuf::from(path)).map_err(to_py_value_error)?,
        })
    }

    /// Open an in-memory SQLite store for deterministic tests.
    #[staticmethod]
    fn in_memory() -> PyResult<Self> {
        Ok(Self {
            inner: SqliteSessionStore::in_memory().map_err(to_py_value_error)?,
        })
    }

    /// Apply pending native SQLite migrations.
    #[staticmethod]
    fn migrate(path: String, py: Python<'_>) -> PyResult<Py<PyAny>> {
        serialize_to_py(
            py,
            &migrate_sqlite_database(PathBuf::from(path)).map_err(to_py_value_error)?,
        )
    }

    /// Return native migration status for a SQLite database.
    #[staticmethod]
    fn migration_status(path: String, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let status = sqlite_migration_status(PathBuf::from(path)).map_err(to_py_value_error)?;
        serialize_to_py(py, &migration_status_json(status))
    }

    fn commit_run_evidence(
        &self,
        py: Python<'_>,
        commit: &Bound<'_, PyAny>,
    ) -> PyResult<Py<PyAny>> {
        let commit = parse_record::<RunEvidenceCommit>(py, commit, "run evidence commit")?;
        let store = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                store
                    .commit_run_evidence(commit)
                    .await
                    .map_err(store_error_to_future)
            },
            |py, run| serialize_to_py(py, &run),
        )
    }

    fn commit_checkpoint(
        &self,
        py: Python<'_>,
        session_id: String,
        checkpoint: &Bound<'_, PyAny>,
    ) -> PyResult<Py<PyAny>> {
        let session_id = SessionId::from_string(session_id);
        let checkpoint = parse_record::<AgentCheckpoint>(py, checkpoint, "checkpoint")?;
        let store = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                store
                    .commit_checkpoint(&session_id, checkpoint)
                    .await
                    .map_err(store_error_to_future)
            },
            |py, ()| Ok(py.None()),
        )
    }

    fn claim_hitl_resume(&self, py: Python<'_>, claim: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        let claim = parse_record::<HitlResumeClaim>(py, claim, "HITL resume claim")?;
        let store = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                store
                    .claim_hitl_resume(claim)
                    .await
                    .map_err(store_error_to_future)
            },
            |py, ()| Ok(py.None()),
        )
    }

    fn mark_hitl_resume_started(
        &self,
        py: Python<'_>,
        session_id: String,
        run_id: String,
        claim_id: String,
    ) -> PyResult<Py<PyAny>> {
        let session_id = SessionId::from_string(session_id);
        let run_id = RunId::from_string(run_id);
        let store = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                store
                    .mark_hitl_resume_started(&session_id, &run_id, &claim_id)
                    .await
                    .map_err(store_error_to_future)
            },
            |py, ()| Ok(py.None()),
        )
    }

    fn release_hitl_resume_claim(
        &self,
        py: Python<'_>,
        session_id: String,
        run_id: String,
        claim_id: String,
    ) -> PyResult<Py<PyAny>> {
        let session_id = SessionId::from_string(session_id);
        let run_id = RunId::from_string(run_id);
        let store = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                store
                    .release_hitl_resume_claim(&session_id, &run_id, &claim_id)
                    .await
                    .map_err(store_error_to_future)
            },
            |py, ()| Ok(py.None()),
        )
    }

    fn pending_stream_publications(
        &self,
        py: Python<'_>,
        session_id: String,
    ) -> PyResult<Py<PyAny>> {
        let session_id = SessionId::from_string(session_id);
        let store = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                store
                    .pending_stream_publications(&session_id)
                    .await
                    .map_err(store_error_to_future)
            },
            |py, publications| serialize_to_py(py, &publications),
        )
    }

    fn acknowledge_stream_publication(
        &self,
        py: Python<'_>,
        publication_id: String,
        target: String,
    ) -> PyResult<Py<PyAny>> {
        let target =
            parse_json_value::<StreamPublicationTarget>(json!(target), "publication target")?;
        let store = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                store
                    .acknowledge_stream_publication(&publication_id, target)
                    .await
                    .map_err(store_error_to_future)
            },
            |py, ()| Ok(py.None()),
        )
    }

    fn save_session(&self, py: Python<'_>, record: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        let record = parse_record::<SessionRecord>(py, record, "session record")?;
        let store = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                store
                    .save_session(record)
                    .await
                    .map_err(store_error_to_future)?;
                Ok(())
            },
            |py, ()| Ok(py.None()),
        )
    }

    fn load_session(&self, py: Python<'_>, session_id: String) -> PyResult<Py<PyAny>> {
        let session_id = SessionId::from_string(session_id);
        let store = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                store
                    .load_session(&session_id)
                    .await
                    .map_err(store_error_to_future)
            },
            |py, record| serialize_to_py(py, &record),
        )
    }

    #[pyo3(signature = (filter=None))]
    fn list_sessions(
        &self,
        py: Python<'_>,
        filter: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        let filter = parse_session_filter(py, filter)?;
        let store = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                store
                    .list_sessions(filter)
                    .await
                    .map_err(store_error_to_future)
            },
            |py, records| serialize_to_py(py, &records),
        )
    }

    fn update_session_status(
        &self,
        py: Python<'_>,
        session_id: String,
        status: String,
    ) -> PyResult<Py<PyAny>> {
        let session_id = SessionId::from_string(session_id);
        let status = parse_json_value::<SessionStatus>(json!(status), "session status")?;
        let store = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                store
                    .update_session_status(&session_id, status)
                    .await
                    .map_err(store_error_to_future)?;
                Ok(())
            },
            |py, ()| Ok(py.None()),
        )
    }

    fn save_context_state(
        &self,
        py: Python<'_>,
        session_id: String,
        state: &Bound<'_, PyAny>,
    ) -> PyResult<Py<PyAny>> {
        let session_id = SessionId::from_string(session_id);
        let state = parse_record(py, state, "context state")?;
        let store = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                store
                    .save_context_state(&session_id, state)
                    .await
                    .map_err(store_error_to_future)?;
                Ok(())
            },
            |py, ()| Ok(py.None()),
        )
    }

    fn save_environment_state(
        &self,
        py: Python<'_>,
        session_id: String,
        environment_state: &Bound<'_, PyAny>,
    ) -> PyResult<Py<PyAny>> {
        let session_id = SessionId::from_string(session_id);
        let environment_state =
            parse_record::<EnvironmentStateRef>(py, environment_state, "environment state")?;
        let store = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                store
                    .save_environment_state(&session_id, environment_state)
                    .await
                    .map_err(store_error_to_future)?;
                Ok(())
            },
            |py, ()| Ok(py.None()),
        )
    }

    fn append_run(&self, py: Python<'_>, record: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        let record = parse_record::<RunRecord>(py, record, "run record")?;
        let store = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                store
                    .append_run(record)
                    .await
                    .map_err(store_error_to_future)?;
                Ok(())
            },
            |py, ()| Ok(py.None()),
        )
    }

    fn append_run_allocated(
        &self,
        py: Python<'_>,
        record: &Bound<'_, PyAny>,
    ) -> PyResult<Py<PyAny>> {
        let record = parse_record::<RunRecord>(py, record, "run record")?;
        let store = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                store
                    .append_run_allocated(record)
                    .map_err(store_error_to_future)
            },
            |py, record| serialize_to_py(py, &record),
        )
    }

    fn load_run(&self, py: Python<'_>, session_id: String, run_id: String) -> PyResult<Py<PyAny>> {
        let session_id = SessionId::from_string(session_id);
        let run_id = RunId::from_string(run_id);
        let store = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                store
                    .load_run(&session_id, &run_id)
                    .await
                    .map_err(store_error_to_future)
            },
            |py, record| serialize_to_py(py, &record),
        )
    }

    fn list_runs(&self, py: Python<'_>, session_id: String) -> PyResult<Py<PyAny>> {
        let session_id = SessionId::from_string(session_id);
        let store = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                store
                    .list_runs(&session_id)
                    .await
                    .map_err(store_error_to_future)
            },
            |py, records| serialize_to_py(py, &records),
        )
    }

    #[pyo3(signature = (session_id, run_id, status, output_preview=None))]
    fn update_run_status(
        &self,
        py: Python<'_>,
        session_id: String,
        run_id: String,
        status: String,
        output_preview: Option<String>,
    ) -> PyResult<Py<PyAny>> {
        let session_id = SessionId::from_string(session_id);
        let run_id = RunId::from_string(run_id);
        let status = parse_json_value::<RunStatus>(json!(status), "run status")?;
        let store = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                store
                    .update_run_status(&session_id, &run_id, status, output_preview)
                    .await
                    .map_err(store_error_to_future)?;
                Ok(())
            },
            |py, ()| Ok(py.None()),
        )
    }

    fn append_checkpoint(
        &self,
        py: Python<'_>,
        session_id: String,
        checkpoint: &Bound<'_, PyAny>,
    ) -> PyResult<Py<PyAny>> {
        let session_id = SessionId::from_string(session_id);
        let checkpoint = parse_record::<AgentCheckpoint>(py, checkpoint, "checkpoint")?;
        let store = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                store
                    .append_checkpoint(&session_id, checkpoint)
                    .await
                    .map_err(store_error_to_future)?;
                Ok(())
            },
            |py, ()| Ok(py.None()),
        )
    }

    fn load_checkpoints(
        &self,
        py: Python<'_>,
        session_id: String,
        run_id: String,
    ) -> PyResult<Py<PyAny>> {
        let session_id = SessionId::from_string(session_id);
        let run_id = RunId::from_string(run_id);
        let store = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                store
                    .load_checkpoints(&session_id, &run_id)
                    .await
                    .map_err(store_error_to_future)
            },
            |py, records| serialize_to_py(py, &records),
        )
    }

    fn append_stream_records(
        &self,
        py: Python<'_>,
        session_id: String,
        run_id: String,
        records: &Bound<'_, PyAny>,
    ) -> PyResult<Py<PyAny>> {
        let session_id = SessionId::from_string(session_id);
        let run_id = RunId::from_string(run_id);
        let records = parse_record::<Vec<AgentStreamRecord>>(py, records, "stream records")?;
        let store = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                store
                    .append_stream_records(&session_id, &run_id, records)
                    .await
                    .map_err(store_error_to_future)?;
                Ok(())
            },
            |py, ()| Ok(py.None()),
        )
    }

    #[pyo3(signature = (session_id, run_id, after_sequence=None))]
    fn replay_stream_records(
        &self,
        py: Python<'_>,
        session_id: String,
        run_id: String,
        after_sequence: Option<usize>,
    ) -> PyResult<Py<PyAny>> {
        let session_id = SessionId::from_string(session_id);
        let run_id = RunId::from_string(run_id);
        let store = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                store
                    .replay_stream_records_after(&session_id, &run_id, after_sequence)
                    .await
                    .map_err(store_error_to_future)
            },
            |py, records| serialize_to_py(py, &records),
        )
    }

    fn save_stream_cursor(
        &self,
        py: Python<'_>,
        session_id: String,
        run_id: String,
        cursor: &Bound<'_, PyAny>,
    ) -> PyResult<Py<PyAny>> {
        let session_id = SessionId::from_string(session_id);
        let run_id = RunId::from_string(run_id);
        let cursor = parse_record::<StreamCursorRef>(py, cursor, "stream cursor")?;
        let store = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                store
                    .save_stream_cursor(&session_id, &run_id, cursor)
                    .await
                    .map_err(store_error_to_future)?;
                Ok(())
            },
            |py, ()| Ok(py.None()),
        )
    }

    fn append_approval(&self, py: Python<'_>, record: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        let record = parse_record::<ApprovalRecord>(py, record, "approval record")?;
        let store = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                store
                    .append_approval(record)
                    .await
                    .map_err(store_error_to_future)?;
                Ok(())
            },
            |py, ()| Ok(py.None()),
        )
    }

    fn load_approvals(
        &self,
        py: Python<'_>,
        session_id: String,
        run_id: String,
    ) -> PyResult<Py<PyAny>> {
        let session_id = SessionId::from_string(session_id);
        let run_id = RunId::from_string(run_id);
        let store = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                store
                    .load_approvals(&session_id, &run_id)
                    .await
                    .map_err(store_error_to_future)
            },
            |py, records| serialize_to_py(py, &records),
        )
    }

    fn append_deferred_tool(
        &self,
        py: Python<'_>,
        record: &Bound<'_, PyAny>,
    ) -> PyResult<Py<PyAny>> {
        let record = parse_record::<DeferredToolRecord>(py, record, "deferred tool record")?;
        let store = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                store
                    .append_deferred_tool(record)
                    .await
                    .map_err(store_error_to_future)?;
                Ok(())
            },
            |py, ()| Ok(py.None()),
        )
    }

    fn load_deferred_tools(
        &self,
        py: Python<'_>,
        session_id: String,
        run_id: String,
    ) -> PyResult<Py<PyAny>> {
        let session_id = SessionId::from_string(session_id);
        let run_id = RunId::from_string(run_id);
        let store = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                store
                    .load_deferred_tools(&session_id, &run_id)
                    .await
                    .map_err(store_error_to_future)
            },
            |py, records| serialize_to_py(py, &records),
        )
    }

    fn resume_snapshot(
        &self,
        py: Python<'_>,
        session_id: String,
        run_id: String,
    ) -> PyResult<Py<PyAny>> {
        let session_id = SessionId::from_string(session_id);
        let run_id = RunId::from_string(run_id);
        let store = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                store
                    .resume_snapshot(&session_id, &run_id)
                    .await
                    .map_err(store_error_to_future)
            },
            |py, snapshot| serialize_to_py(py, &snapshot),
        )
    }

    fn compact_run_trace(
        &self,
        py: Python<'_>,
        session_id: String,
        run_id: String,
    ) -> PyResult<Py<PyAny>> {
        let session_id = SessionId::from_string(session_id);
        let run_id = RunId::from_string(run_id);
        let store = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                store
                    .compact_run_trace(&session_id, &run_id)
                    .await
                    .map_err(store_error_to_future)
            },
            |py, trace| serialize_to_py(py, &trace),
        )
    }

    fn compact_session_trace(&self, py: Python<'_>, session_id: String) -> PyResult<Py<PyAny>> {
        let session_id = SessionId::from_string(session_id);
        let store = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                store
                    .compact_session_trace(&session_id)
                    .await
                    .map_err(store_error_to_future)
            },
            |py, trace| serialize_to_py(py, &trace),
        )
    }
}

impl PySqliteReplayEventLog {
    pub(crate) fn replay_event_log(&self) -> Arc<dyn ReplayEventLog> {
        Arc::new(self.inner.clone())
    }
}

#[pymethods]
impl PySqliteReplayEventLog {
    #[new]
    fn new(path: String) -> PyResult<Self> {
        Self::open(path)
    }

    /// Open or create a SQLite replay event log and apply native migrations.
    #[staticmethod]
    fn open(path: String) -> PyResult<Self> {
        Ok(Self {
            inner: SqliteReplayEventLog::open(PathBuf::from(path)).map_err(replay_error_to_py)?,
        })
    }

    /// Open an in-memory SQLite replay event log for deterministic tests.
    #[staticmethod]
    fn in_memory() -> PyResult<Self> {
        Ok(Self {
            inner: SqliteReplayEventLog::in_memory().map_err(replay_error_to_py)?,
        })
    }

    fn append(
        &self,
        py: Python<'_>,
        scope: String,
        event: &Bound<'_, PyAny>,
    ) -> PyResult<Py<PyAny>> {
        let scope = ReplayScope::from_string(scope);
        let event = parse_record::<ReplayEvent>(py, event, "replay event")?;
        let log = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                log.append(scope, event)
                    .await
                    .map_err(replay_error_to_future)?;
                Ok(())
            },
            |py, ()| Ok(py.None()),
        )
    }

    #[pyo3(signature = (scope, cursor=None, limit=None))]
    fn replay_after(
        &self,
        py: Python<'_>,
        scope: String,
        cursor: Option<&Bound<'_, PyAny>>,
        limit: Option<usize>,
    ) -> PyResult<Py<PyAny>> {
        let scope = ReplayScope::from_string(scope);
        let cursor = parse_optional_record::<ReplayCursor>(py, cursor, "replay cursor")?;
        let log = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                log.replay_after(&scope, cursor, limit)
                    .await
                    .map_err(replay_error_to_future)
            },
            |py, events| serialize_to_py(py, &events),
        )
    }

    fn compact_snapshot(&self, py: Python<'_>, scope: String) -> PyResult<Py<PyAny>> {
        let scope = ReplayScope::from_string(scope);
        let log = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                log.compact_snapshot(&scope)
                    .await
                    .map_err(replay_error_to_future)
            },
            |py, snapshot| serialize_to_py(py, &snapshot),
        )
    }

    fn save_snapshot(
        &self,
        py: Python<'_>,
        scope: String,
        snapshot: &Bound<'_, PyAny>,
    ) -> PyResult<Py<PyAny>> {
        let scope = ReplayScope::from_string(scope);
        let snapshot = parse_record::<ReplaySnapshot>(py, snapshot, "replay snapshot")?;
        let log = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                log.save_snapshot(scope, snapshot)
                    .map_err(replay_error_to_future)?;
                Ok(())
            },
            |py, ()| Ok(py.None()),
        )
    }
}

impl PySqliteStreamArchive {
    pub(crate) fn stream_archive(&self) -> Arc<dyn StreamArchive> {
        Arc::new(self.inner.clone())
    }
}

#[pymethods]
impl PySqliteStreamArchive {
    #[new]
    fn new(path: String) -> PyResult<Self> {
        Self::open(path)
    }

    /// Open or create a SQLite stream archive and apply native migrations.
    #[staticmethod]
    fn open(path: String) -> PyResult<Self> {
        Ok(Self {
            inner: SqliteStreamArchive::open(PathBuf::from(path)).map_err(replay_error_to_py)?,
        })
    }

    /// Open an in-memory SQLite stream archive for deterministic tests.
    #[staticmethod]
    fn in_memory() -> PyResult<Self> {
        Ok(Self {
            inner: SqliteStreamArchive::in_memory().map_err(replay_error_to_py)?,
        })
    }

    fn append_raw_records(
        &self,
        py: Python<'_>,
        session_id: String,
        run_id: String,
        records: &Bound<'_, PyAny>,
    ) -> PyResult<Py<PyAny>> {
        let session_id = SessionId::from_string(session_id);
        let run_id = RunId::from_string(run_id);
        let records = parse_record::<Vec<AgentStreamRecord>>(py, records, "raw stream records")?;
        let archive = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                archive
                    .append_raw_records(&session_id, &run_id, records)
                    .await
                    .map_err(replay_error_to_future)?;
                Ok(())
            },
            |py, ()| Ok(py.None()),
        )
    }

    #[pyo3(signature = (session_id, run_id, cursor=None))]
    fn replay_raw_after(
        &self,
        py: Python<'_>,
        session_id: String,
        run_id: String,
        cursor: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        let session_id = SessionId::from_string(session_id);
        let run_id = RunId::from_string(run_id);
        let cursor = parse_optional_record::<ReplayCursor>(py, cursor, "replay cursor")?;
        let archive = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                archive
                    .replay_raw_after(&session_id, &run_id, cursor)
                    .await
                    .map_err(replay_error_to_future)
            },
            |py, records| serialize_to_py(py, &records),
        )
    }

    fn append_display_messages(
        &self,
        py: Python<'_>,
        scope: String,
        messages: &Bound<'_, PyAny>,
    ) -> PyResult<Py<PyAny>> {
        let scope = ReplayScope::from_string(scope);
        let messages = parse_record::<Vec<DisplayMessage>>(py, messages, "display messages")?;
        let archive = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                archive
                    .append_display_messages(scope, messages)
                    .await
                    .map_err(replay_error_to_future)?;
                Ok(())
            },
            |py, ()| Ok(py.None()),
        )
    }

    #[pyo3(signature = (scope, cursor=None))]
    fn replay_display_after(
        &self,
        py: Python<'_>,
        scope: String,
        cursor: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        let scope = ReplayScope::from_string(scope);
        let cursor = parse_optional_record::<ReplayCursor>(py, cursor, "replay cursor")?;
        let archive = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                archive
                    .replay_display_after(&scope, cursor)
                    .await
                    .map_err(replay_error_to_future)
            },
            |py, messages| serialize_to_py(py, &messages),
        )
    }

    fn append_snapshot(
        &self,
        py: Python<'_>,
        scope: String,
        snapshot: &Bound<'_, PyAny>,
    ) -> PyResult<Py<PyAny>> {
        let scope = ReplayScope::from_string(scope);
        let snapshot = parse_record::<ReplaySnapshot>(py, snapshot, "replay snapshot")?;
        let archive = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                archive
                    .append_snapshot(scope, snapshot)
                    .await
                    .map_err(replay_error_to_future)?;
                Ok(())
            },
            |py, ()| Ok(py.None()),
        )
    }

    fn latest_snapshot(&self, py: Python<'_>, scope: String) -> PyResult<Py<PyAny>> {
        let scope = ReplayScope::from_string(scope);
        let archive = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                archive
                    .latest_snapshot(&scope)
                    .await
                    .map_err(replay_error_to_future)
            },
            |py, snapshot| serialize_to_py(py, &snapshot),
        )
    }

    fn cursor_range(&self, py: Python<'_>, scope: String) -> PyResult<Py<PyAny>> {
        let scope = ReplayScope::from_string(scope);
        let archive = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                archive
                    .cursor_range(&scope)
                    .await
                    .map_err(replay_error_to_future)
                    .map(|range| range.map(|(first, last)| json!({"first": first, "last": last})))
            },
            |py, range| serialize_to_py(py, &range),
        )
    }
}

pub(crate) fn extract_session_store(
    value: Option<&Bound<'_, PyAny>>,
) -> PyResult<Option<Arc<dyn SessionStore>>> {
    match value {
        Some(value) if !value.is_none() => extract_session_store_value(value).map(Some),
        Some(_) | None => Ok(None),
    }
}

pub(crate) fn extract_replay_event_log(
    value: Option<&Bound<'_, PyAny>>,
) -> PyResult<Option<Arc<dyn ReplayEventLog>>> {
    match value {
        Some(value) if !value.is_none() => extract_replay_event_log_value(value).map(Some),
        Some(_) | None => Ok(None),
    }
}

pub(crate) fn extract_stream_archive(
    value: Option<&Bound<'_, PyAny>>,
) -> PyResult<Option<Arc<dyn StreamArchive>>> {
    match value {
        Some(value) if !value.is_none() => extract_stream_archive_value(value).map(Some),
        Some(_) | None => Ok(None),
    }
}

fn extract_session_store_value(value: &Bound<'_, PyAny>) -> PyResult<Arc<dyn SessionStore>> {
    if let Ok(store) = value.extract::<PyRef<'_, PySqliteSessionStore>>() {
        return Ok(store.session_store());
    }
    if let Ok(store) = value.extract::<PyRef<'_, PyPythonSessionStore>>() {
        return Ok(store.session_store());
    }
    if let Ok(to_native) = value.getattr("to_native") {
        let native = to_native.call0()?;
        return extract_session_store_value(&native);
    }
    if let Ok(native) = value.getattr("_native") {
        return extract_session_store_value(&native);
    }
    Err(PyValueError::new_err(
        "session_store must be a SessionStore or native session store",
    ))
}

fn extract_replay_event_log_value(value: &Bound<'_, PyAny>) -> PyResult<Arc<dyn ReplayEventLog>> {
    if let Ok(log) = value.extract::<PyRef<'_, PySqliteReplayEventLog>>() {
        return Ok(log.replay_event_log());
    }
    if let Ok(to_native) = value.getattr("to_native") {
        let native = to_native.call0()?;
        return extract_replay_event_log_value(&native);
    }
    if let Ok(native) = value.getattr("_native") {
        return extract_replay_event_log_value(&native);
    }
    Err(PyValueError::new_err(
        "replay_event_log must be a SqliteReplayEventLog or native replay event log",
    ))
}

fn extract_stream_archive_value(value: &Bound<'_, PyAny>) -> PyResult<Arc<dyn StreamArchive>> {
    if let Ok(archive) = value.extract::<PyRef<'_, PySqliteStreamArchive>>() {
        return Ok(archive.stream_archive());
    }
    if let Ok(to_native) = value.getattr("to_native") {
        let native = to_native.call0()?;
        return extract_stream_archive_value(&native);
    }
    if let Ok(native) = value.getattr("_native") {
        return extract_stream_archive_value(&native);
    }
    Err(PyValueError::new_err(
        "stream_archive must be a SqliteStreamArchive or native stream archive",
    ))
}

fn parse_record<T>(py: Python<'_>, value: &Bound<'_, PyAny>, label: &str) -> PyResult<T>
where
    T: DeserializeOwned,
{
    let value = if let Ok(to_dict) = value.getattr("to_dict") {
        py_to_json(py, &to_dict.call0()?)?
    } else {
        py_to_json(py, value)?
    };
    serde_json::from_value::<T>(value)
        .map_err(|error| PyValueError::new_err(format!("invalid {label}: {error}")))
}

fn parse_json_value<T>(value: Value, label: &str) -> PyResult<T>
where
    T: DeserializeOwned,
{
    serde_json::from_value::<T>(value)
        .map_err(|error| PyValueError::new_err(format!("invalid {label}: {error}")))
}

fn parse_session_filter(
    py: Python<'_>,
    value: Option<&Bound<'_, PyAny>>,
) -> PyResult<SessionFilter> {
    let Some(value) = value.filter(|value| !value.is_none()) else {
        return Ok(SessionFilter::default());
    };
    let value = py_to_json(py, value)?;
    let Some(object) = value.as_object() else {
        return Err(PyValueError::new_err("session filter must be an object"));
    };
    let status = object
        .get("status")
        .filter(|value| !value.is_null())
        .cloned()
        .map(|value| parse_json_value::<SessionStatus>(value, "session filter status"))
        .transpose()?;
    let profile = object
        .get("profile")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let workspace = object
        .get("workspace")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let limit = object
        .get("limit")
        .and_then(Value::as_u64)
        .map(|value| value as usize);
    Ok(SessionFilter {
        status,
        profile,
        workspace,
        limit,
    })
}

fn parse_optional_record<T>(
    py: Python<'_>,
    value: Option<&Bound<'_, PyAny>>,
    label: &str,
) -> PyResult<Option<T>>
where
    T: DeserializeOwned,
{
    match value {
        Some(value) if !value.is_none() => parse_record(py, value, label).map(Some),
        Some(_) | None => Ok(None),
    }
}

fn store_error_to_future(error: SessionStoreError) -> PyFutureError {
    PyFutureError::State(error.to_string())
}

fn to_py_value_error(error: SessionStoreError) -> PyErr {
    PyValueError::new_err(error.to_string())
}

fn py_err_to_store_error(operation: &str, error: PyErr) -> SessionStoreError {
    let message = error.to_string();
    let lowered = message.to_ascii_lowercase();
    if lowered.contains("not found")
        || lowered.contains("unknown session")
        || lowered.contains("unknown run")
    {
        SessionStoreError::NotFound(message)
    } else {
        SessionStoreError::Failed(format!("{operation} failed: {message}"))
    }
}

fn replay_error_to_future(error: ReplayError) -> PyFutureError {
    PyFutureError::State(error.to_string())
}

fn replay_error_to_py(error: ReplayError) -> PyErr {
    PyValueError::new_err(error.to_string())
}

fn migration_status_json(status: SqliteMigrationStatus) -> serde_json::Value {
    json!({
        "migration_table_exists": status.migration_table_exists,
        "applied": status
            .applied
            .into_iter()
            .map(|migration| {
                json!({
                    "id": migration.id,
                    "description": migration.description,
                    "checksum": migration.checksum,
                    "applied_at": migration.applied_at,
                })
            })
            .collect::<Vec<_>>(),
        "pending": status
            .pending
            .into_iter()
            .map(|migration| {
                json!({
                    "id": migration.id,
                    "description": migration.description,
                    "checksum": migration.checksum,
                })
            })
            .collect::<Vec<_>>(),
        "latest_migration": status.latest_migration,
        "current": status.current,
    })
}

fn session_filter_json(filter: SessionFilter) -> Value {
    json!({
        "status": filter.status,
        "profile": filter.profile,
        "workspace": filter.workspace,
        "limit": filter.limit,
    })
}
