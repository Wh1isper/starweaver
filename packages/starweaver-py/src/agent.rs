//! Python wrappers for Starweaver agents, runs, streams, and sessions.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use pyo3::{
    exceptions::{PyRuntimeError, PyValueError},
    prelude::*,
};
use serde_json::{Map, Value};
use starweaver_agent::{
    AgentBuilder, AgentHitlError, AgentHitlResults, AgentLiveStreamResult, AgentRunOptions,
    AgentSession, AgentStreamController, AgentStreamError, AgentStreamHandle, ResumableState,
};
use starweaver_model::ToolReturnPart;
use starweaver_runtime::{Agent as RuntimeAgent, AgentResult, AgentStreamResult, RunStatus};
use starweaver_session::{DeferredToolResults, ToolApprovalDecision};
use starweaver_tools::ToolRegistry;
use tokio::sync::Mutex;

use crate::{
    capability::PyCapabilityBundle,
    conversion::{json_to_py, py_to_json, serialize_to_py},
    model::{extract_model_settings, extract_request_params},
    output::{extract_output_policy, extract_output_schema},
    runtime::{PyFutureError, enter_runtime, spawn_py_future},
    stream::PyStreamEvent,
    subagent::{PySubagent, parse_delegation_mode},
    testing::py_model_from_any,
    tool::PyPythonTool,
};

/// Python wrapper around the Starweaver runtime agent.
#[pyclass(name = "Agent", skip_from_py_object)]
#[derive(Clone)]
pub struct PyAgent {
    inner: RuntimeAgent,
}

impl PyAgent {
    pub(crate) fn runtime_agent(&self) -> RuntimeAgent {
        self.inner.clone()
    }
}

#[pymethods]
impl PyAgent {
    #[new]
    #[pyo3(signature = (model, tools=None, instructions=None, name=None, model_settings=None, request_params=None, output_schema=None, output_policy=None, subagents=None, subagent_delegation_mode=None, capability_bundles=None))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        py: Python<'_>,
        model: &Bound<'_, PyAny>,
        tools: Option<Vec<Py<PyPythonTool>>>,
        instructions: Option<Vec<String>>,
        name: Option<String>,
        model_settings: Option<&Bound<'_, PyAny>>,
        request_params: Option<&Bound<'_, PyAny>>,
        output_schema: Option<&Bound<'_, PyAny>>,
        output_policy: Option<&Bound<'_, PyAny>>,
        subagents: Option<Vec<Py<PySubagent>>>,
        subagent_delegation_mode: Option<String>,
        capability_bundles: Option<Vec<Py<PyCapabilityBundle>>>,
    ) -> PyResult<Self> {
        if output_schema.is_some() && output_policy.is_some() {
            return Err(PyValueError::new_err(
                "pass output_schema or output_policy, not both",
            ));
        }
        let mut builder = AgentBuilder::new(py_model_from_any(model)?);
        if let Some(name) = name {
            builder = builder.agent_name(name);
        }
        if let Some(instructions) = instructions {
            for instruction in instructions {
                builder = builder.instruction(instruction);
            }
        }
        for tool in tools.unwrap_or_default() {
            builder = builder.tool(tool.borrow(py).dyn_tool());
        }
        if let Some(settings) = extract_model_settings(py, model_settings)? {
            builder = builder.model_settings(settings);
        }
        if let Some(params) = extract_request_params(py, request_params)? {
            builder = builder.request_params(params);
        }
        if let Some(schema) = extract_output_schema(py, output_schema)? {
            builder = builder.output_schema(schema);
        }
        if let Some(policy) = extract_output_policy(py, output_policy)? {
            builder = builder.output_policy(policy);
        }
        for subagent in subagents.unwrap_or_default() {
            builder = builder.subagent(subagent.borrow(py).config());
        }
        builder =
            builder.subagent_delegation_mode(parse_delegation_mode(subagent_delegation_mode)?);
        for bundle in capability_bundles.unwrap_or_default() {
            builder = builder.capability_bundle(bundle.borrow(py).bundle());
        }
        Ok(Self {
            inner: builder.build(),
        })
    }

    fn run(&self, py: Python<'_>, prompt: String) -> PyResult<Py<PyAny>> {
        let agent = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                agent
                    .run(prompt)
                    .await
                    .map_err(PyFutureError::from_agent_error)
            },
            |py, result| Ok(Py::new(py, PyRunResult::from_agent_result(result)?)?.into_any()),
        )
    }

    fn run_stream_collect(&self, py: Python<'_>, prompt: String) -> PyResult<Py<PyAny>> {
        let agent = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                agent
                    .run_stream(prompt)
                    .await
                    .map_err(PyFutureError::from_agent_error)
            },
            |py, result| Ok(Py::new(py, PyStreamRunResult::from_stream_result(result)?)?.into_any()),
        )
    }

    #[pyo3(signature = (prompt, instructions=None, tools=None, replace_tools=false, model_settings=None, request_params=None, output_schema=None, output_policy=None))]
    #[allow(clippy::too_many_arguments)]
    fn stream(
        &self,
        py: Python<'_>,
        prompt: String,
        instructions: Option<Vec<String>>,
        tools: Option<Vec<Py<PyPythonTool>>>,
        replace_tools: bool,
        model_settings: Option<&Bound<'_, PyAny>>,
        request_params: Option<&Bound<'_, PyAny>>,
        output_schema: Option<&Bound<'_, PyAny>>,
        output_policy: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<PyAgentStream> {
        let mut session = AgentSession::new(self.inner.clone());
        let options = py_run_options(
            py,
            instructions,
            tools,
            replace_tools,
            model_settings,
            request_params,
            output_schema,
            output_policy,
        )?;
        let handle = enter_runtime(|| Ok(session.stream_with_options(prompt, options)))?;
        Ok(PyAgentStream::new(
            handle,
            Some(Arc::new(Mutex::new(session))),
            None,
        ))
    }

    fn new_session(&self) -> PySession {
        PySession {
            inner: Arc::new(Mutex::new(AgentSession::new(self.inner.clone()))),
            busy: Arc::new(AtomicBool::new(false)),
        }
    }

    fn session_from_state(&self, py: Python<'_>, state: &Bound<'_, PyAny>) -> PyResult<PySession> {
        let state: ResumableState = serde_json::from_value(py_to_json(py, state)?)
            .map_err(|error| PyValueError::new_err(format!("invalid session state: {error}")))?;
        Ok(PySession {
            inner: Arc::new(Mutex::new(AgentSession::from_state(
                self.inner.clone(),
                state,
            ))),
            busy: Arc::new(AtomicBool::new(false)),
        })
    }
}

/// Live Python stream handle over Starweaver stream records.
#[pyclass(name = "AgentStream", skip_from_py_object)]
#[derive(Clone)]
pub struct PyAgentStream {
    handle: Arc<Mutex<Option<AgentStreamHandle>>>,
    controller: AgentStreamController,
    session: Option<Arc<Mutex<AgentSession>>>,
    session_lease: Option<Arc<PySessionOperationLease>>,
}

impl PyAgentStream {
    fn new(
        handle: AgentStreamHandle,
        session: Option<Arc<Mutex<AgentSession>>>,
        session_lease: Option<Arc<PySessionOperationLease>>,
    ) -> Self {
        let controller = handle.controller();
        Self {
            handle: Arc::new(Mutex::new(Some(handle))),
            controller,
            session,
            session_lease,
        }
    }
}

impl Drop for PyAgentStream {
    fn drop(&mut self) {
        self.controller.interrupt();
        if let Some(lease) = &self.session_lease {
            lease.release();
        }
    }
}

#[pymethods]
impl PyAgentStream {
    fn recv(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let handle = self.handle.clone();
        spawn_py_future(
            py,
            async move {
                let mut guard = handle.lock().await;
                let Some(handle) = guard.as_mut() else {
                    return Ok(None);
                };
                Ok(handle.recv().await)
            },
            |py, record| match record {
                Some(record) => Ok(Py::new(py, PyStreamEvent::from_record(&record)?)?.into_any()),
                None => Ok(py.None()),
            },
        )
    }

    fn interrupt(&self) -> PyResult<()> {
        self.controller.interrupt();
        Ok(())
    }

    fn join(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let handle = self.handle.clone();
        let session = self.session.clone();
        let session_lease = self.session_lease.clone();
        spawn_py_future(
            py,
            async move {
                let result = async {
                    let stream = {
                        let mut guard = handle.lock().await;
                        guard.take()
                    };
                    let Some(stream) = stream else {
                        return Err(PyFutureError::Stream(
                            "stream has already completed".to_string(),
                        ));
                    };
                    let result = if let Some(session) = session {
                        let mut session = session.lock().await;
                        stream
                            .finish_into_session(&mut session)
                            .await
                            .map_err(stream_error_to_py)?
                    } else {
                        stream.join().await.map_err(stream_error_to_py)?
                    };
                    Ok(result)
                }
                .await;
                if let Some(lease) = &session_lease {
                    lease.release();
                }
                result
            },
            |py, result| Ok(Py::new(py, PyStreamRunResult::from_live_result(result)?)?.into_any()),
        )
    }

    fn result(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let handle = self.handle.clone();
        let session = self.session.clone();
        let session_lease = self.session_lease.clone();
        spawn_py_future(
            py,
            async move {
                let result = async {
                    let stream = {
                        let mut guard = handle.lock().await;
                        guard.take()
                    };
                    let Some(stream) = stream else {
                        return Err(PyFutureError::Stream(
                            "stream has already completed".to_string(),
                        ));
                    };
                    let result = if let Some(session) = session {
                        let mut session = session.lock().await;
                        stream
                            .finish_into_session(&mut session)
                            .await
                            .map_err(stream_error_to_py)?
                    } else {
                        stream.join().await.map_err(stream_error_to_py)?
                    };
                    Ok(result.result)
                }
                .await;
                if let Some(lease) = &session_lease {
                    lease.release();
                }
                result
            },
            |py, result| Ok(Py::new(py, PyRunResult::from_agent_result(result)?)?.into_any()),
        )
    }

    fn recoverable_state(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let controller = self.controller.clone();
        spawn_py_future(
            py,
            async move { Ok::<_, PyFutureError>(controller.recoverable_state().await) },
            |py, state| serialize_to_py(py, &state),
        )
    }

    fn status(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let guard = self
            .handle
            .try_lock()
            .map_err(|_| PyRuntimeError::new_err("stream is busy"))?;
        let status = guard.as_ref().map_or_else(
            || {
                serde_json::json!({
                    "run_status": "finished",
                    "cancel_requested": self.controller.cancel_requested(),
                })
            },
            stream_status_json,
        );
        json_to_py(py, &status)
    }
}
/// Python run result projection.
#[pyclass(name = "RunResult", skip_from_py_object)]
#[derive(Clone, Debug)]
pub struct PyRunResult {
    output: String,
    structured_output: Option<Value>,
    messages: Value,
    state: Value,
    status: String,
    is_waiting: bool,
    pending_approvals: Vec<Value>,
    pending_deferred: Vec<Value>,
    needs_approval: bool,
}

impl PyRunResult {
    fn from_agent_result(result: AgentResult) -> PyResult<Self> {
        let messages = serde_json::to_value(&result.messages).map_err(to_py_value_error)?;
        let state = serde_json::to_value(&result.state).map_err(to_py_value_error)?;
        let status = run_status_name(result.state.status).to_string();
        let is_waiting = result.state.status == RunStatus::Waiting || result.has_pending_hitl();
        let pending_approvals = result
            .pending_approvals()
            .iter()
            .map(|tool_return| {
                pending_tool_return_projection(
                    tool_return,
                    "approval_id",
                    &format!(
                        "approval_{}_{}",
                        result.state.run_id.as_str(),
                        tool_return.tool_call_id
                    ),
                )
            })
            .collect::<PyResult<Vec<_>>>()?;
        let pending_deferred = result
            .pending_deferred_tools()
            .iter()
            .map(|tool_return| {
                pending_tool_return_projection(
                    tool_return,
                    "deferred_id",
                    &format!(
                        "deferred_{}_{}",
                        result.state.run_id.as_str(),
                        tool_return.tool_call_id
                    ),
                )
            })
            .collect::<PyResult<Vec<_>>>()?;
        let needs_approval = result.has_pending_hitl();
        Ok(Self {
            output: result.output,
            structured_output: result.structured_output,
            messages,
            state,
            status,
            is_waiting,
            pending_approvals,
            pending_deferred,
            needs_approval,
        })
    }
}

#[pymethods]
impl PyRunResult {
    #[getter]
    fn output(&self) -> &str {
        &self.output
    }

    #[getter]
    fn structured_output(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        match &self.structured_output {
            Some(value) => json_to_py(py, value),
            None => Ok(py.None()),
        }
    }

    #[getter]
    fn messages(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        json_to_py(py, &self.messages)
    }

    #[getter]
    fn raw_state(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        json_to_py(py, &self.state)
    }

    #[getter]
    fn status(&self) -> &str {
        &self.status
    }

    #[getter]
    fn is_waiting(&self) -> bool {
        self.is_waiting
    }

    #[getter]
    fn needs_approval(&self) -> bool {
        self.needs_approval
    }

    #[getter]
    fn pending_approvals(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        json_to_py(py, &Value::Array(self.pending_approvals.clone()))
    }

    #[getter]
    fn pending_deferred_tools(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        json_to_py(py, &Value::Array(self.pending_deferred.clone()))
    }

    #[getter]
    fn pending_deferred(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        json_to_py(py, &Value::Array(self.pending_deferred.clone()))
    }
}

/// Result of a collected stream run.
#[pyclass(name = "StreamRunResult", skip_from_py_object)]
#[derive(Clone, Debug)]
pub struct PyStreamRunResult {
    result: PyRunResult,
    events: Vec<PyStreamEvent>,
}

impl PyStreamRunResult {
    fn from_stream_result(result: AgentStreamResult) -> PyResult<Self> {
        let events = result
            .events
            .iter()
            .map(PyStreamEvent::from_record)
            .collect::<PyResult<Vec<_>>>()?;
        Ok(Self {
            result: PyRunResult::from_agent_result(result.result)?,
            events,
        })
    }

    fn from_live_result(result: AgentLiveStreamResult) -> PyResult<Self> {
        Self::from_stream_result(result.into_stream_result())
    }
}

#[pymethods]
impl PyStreamRunResult {
    #[getter]
    fn result(&self) -> PyRunResult {
        self.result.clone()
    }

    #[getter]
    fn events(&self) -> Vec<PyStreamEvent> {
        self.events.clone()
    }
}

/// Python wrapper around a stateful Starweaver agent session.
#[pyclass(name = "AgentSession", skip_from_py_object)]
#[derive(Clone)]
pub struct PySession {
    inner: Arc<Mutex<AgentSession>>,
    busy: Arc<AtomicBool>,
}

impl PySession {
    fn acquire_operation(&self) -> PyResult<Arc<PySessionOperationLease>> {
        PySessionOperationLease::acquire(self.busy.clone())
    }
}

struct PySessionOperationLease {
    busy: Arc<AtomicBool>,
    released: AtomicBool,
}

impl PySessionOperationLease {
    fn acquire(busy: Arc<AtomicBool>) -> PyResult<Arc<Self>> {
        if busy
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err(PyRuntimeError::new_err("session is busy"));
        }
        Ok(Arc::new(Self {
            busy,
            released: AtomicBool::new(false),
        }))
    }

    fn release(&self) {
        if !self.released.swap(true, Ordering::SeqCst) {
            self.busy.store(false, Ordering::SeqCst);
        }
    }
}

impl Drop for PySessionOperationLease {
    fn drop(&mut self) {
        self.release();
    }
}

#[pymethods]
impl PySession {
    fn run(&self, py: Python<'_>, prompt: String) -> PyResult<Py<PyAny>> {
        let lease = self.acquire_operation()?;
        let session = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                let _lease = lease;
                let mut session = session.lock().await;
                session
                    .run(prompt)
                    .await
                    .map_err(PyFutureError::from_agent_error)
            },
            |py, result| Ok(Py::new(py, PyRunResult::from_agent_result(result)?)?.into_any()),
        )
    }

    fn run_stream_collect(&self, py: Python<'_>, prompt: String) -> PyResult<Py<PyAny>> {
        let lease = self.acquire_operation()?;
        let session = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                let _lease = lease;
                let mut session = session.lock().await;
                session
                    .run_stream(prompt)
                    .await
                    .map_err(PyFutureError::from_agent_error)
            },
            |py, result| Ok(Py::new(py, PyStreamRunResult::from_stream_result(result)?)?.into_any()),
        )
    }

    #[pyo3(signature = (prompt, instructions=None, tools=None, replace_tools=false, model_settings=None, request_params=None, output_schema=None, output_policy=None))]
    #[allow(clippy::too_many_arguments)]
    fn stream(
        &self,
        py: Python<'_>,
        prompt: String,
        instructions: Option<Vec<String>>,
        tools: Option<Vec<Py<PyPythonTool>>>,
        replace_tools: bool,
        model_settings: Option<&Bound<'_, PyAny>>,
        request_params: Option<&Bound<'_, PyAny>>,
        output_schema: Option<&Bound<'_, PyAny>>,
        output_policy: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<PyAgentStream> {
        let lease = self.acquire_operation()?;
        let options = py_run_options(
            py,
            instructions,
            tools,
            replace_tools,
            model_settings,
            request_params,
            output_schema,
            output_policy,
        )?;
        let mut session = self
            .inner
            .try_lock()
            .map_err(|_| PyRuntimeError::new_err("session is busy"))?;
        let handle = enter_runtime(|| Ok(session.stream_with_options(prompt, options)))?;
        Ok(PyAgentStream::new(
            handle,
            Some(self.inner.clone()),
            Some(lease),
        ))
    }

    fn export_state(&self, py: Python<'_>, mode: Option<String>) -> PyResult<Py<PyAny>> {
        let _lease = self.acquire_operation()?;
        let mode = mode.unwrap_or_else(|| "curated".to_string());
        let session = self.inner.blocking_lock();
        let state = match mode.as_str() {
            "curated" => session.export_state(),
            "full" => session.export_full_state(),
            other => {
                return Err(PyValueError::new_err(format!(
                    "unsupported export mode: {other}"
                )));
            }
        };
        serialize_to_py(py, &state)
    }

    #[pyo3(signature = (approvals=None, deferred_results=None))]
    fn resume_after_hitl(
        &self,
        py: Python<'_>,
        approvals: Option<&Bound<'_, PyAny>>,
        deferred_results: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        let lease = self.acquire_operation()?;
        let results = parse_hitl_results(py, approvals, deferred_results)?;
        let session = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                let _lease = lease;
                let mut session = session.lock().await;
                session
                    .resume_after_hitl(results)
                    .await
                    .map_err(hitl_error_to_py)
            },
            |py, result| Ok(Py::new(py, PyRunResult::from_agent_result(result)?)?.into_any()),
        )
    }
}

fn parse_hitl_results(
    py: Python<'_>,
    approvals: Option<&Bound<'_, PyAny>>,
    deferred_results: Option<&Bound<'_, PyAny>>,
) -> PyResult<AgentHitlResults> {
    let mut results = AgentHitlResults::new();
    if let Some(approvals) = approvals {
        let Value::Object(approval_map) = py_to_json(py, approvals)? else {
            return Err(PyValueError::new_err("approvals must be a mapping"));
        };
        for (id, value) in approval_map {
            results = results.approval(id, parse_approval_decision(value)?);
        }
    }
    if let Some(deferred_results) = deferred_results {
        let value = py_to_json(py, deferred_results)?;
        let deferred_results: DeferredToolResults =
            serde_json::from_value(value).map_err(|error| {
                PyValueError::new_err(format!("invalid deferred_results payload: {error}"))
            })?;
        results = results.deferred_results(deferred_results);
    }
    Ok(results)
}

fn parse_approval_decision(value: Value) -> PyResult<ToolApprovalDecision> {
    match value {
        Value::Bool(true) => Ok(ToolApprovalDecision::approved()),
        Value::Bool(false) => Ok(ToolApprovalDecision::denied("denied")),
        Value::Object(mut object) => {
            let approved = object
                .remove("approved")
                .and_then(|value| value.as_bool())
                .unwrap_or(true);
            let decided_by = object
                .remove("decided_by")
                .and_then(|value| value.as_str().map(ToOwned::to_owned));
            let reason = object
                .remove("reason")
                .and_then(|value| value.as_str().map(ToOwned::to_owned));
            let override_arguments = object.remove("override_arguments");
            let metadata = match object.remove("metadata") {
                Some(Value::Object(metadata)) => metadata,
                Some(_) => {
                    return Err(PyValueError::new_err("approval metadata must be an object"));
                }
                None => Map::new(),
            };
            if approved {
                Ok(ToolApprovalDecision::Approved {
                    decided_by,
                    reason,
                    override_arguments,
                    metadata,
                })
            } else {
                Ok(ToolApprovalDecision::Denied {
                    decided_by,
                    reason,
                    metadata,
                })
            }
        }
        _ => Err(PyValueError::new_err(
            "approval decision must be a bool or mapping",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
fn py_run_options(
    py: Python<'_>,
    instructions: Option<Vec<String>>,
    tools: Option<Vec<Py<PyPythonTool>>>,
    replace_tools: bool,
    model_settings: Option<&Bound<'_, PyAny>>,
    request_params: Option<&Bound<'_, PyAny>>,
    output_schema: Option<&Bound<'_, PyAny>>,
    output_policy: Option<&Bound<'_, PyAny>>,
) -> PyResult<AgentRunOptions> {
    if output_schema.is_some() && output_policy.is_some() {
        return Err(PyValueError::new_err(
            "pass output_schema or output_policy, not both",
        ));
    }
    let mut options = AgentRunOptions::new();
    for instruction in instructions.unwrap_or_default() {
        options = options.instruction(instruction);
    }
    let mut registry = ToolRegistry::new();
    for tool in tools.unwrap_or_default() {
        registry.insert(tool.borrow(py).dyn_tool());
    }
    if replace_tools {
        options = options.replace_tools();
    }
    if !registry.is_empty() {
        options = options.append_tool_registry(&registry);
    }
    if let Some(settings) = extract_model_settings(py, model_settings)? {
        options = options.model_settings(settings);
    }
    if let Some(params) = extract_request_params(py, request_params)? {
        options = options.request_params(params);
    }
    if let Some(schema) = extract_output_schema(py, output_schema)? {
        options = options.output_policy(starweaver_runtime::OutputPolicy::structured(schema));
    }
    if let Some(policy) = extract_output_policy(py, output_policy)? {
        options = options.output_policy(policy);
    }
    Ok(options)
}

fn to_py_value_error(error: serde_json::Error) -> PyErr {
    PyValueError::new_err(format!("serialization failed: {error}"))
}

fn run_status_name(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Starting => "starting",
        RunStatus::Running => "running",
        RunStatus::Waiting => "waiting",
        RunStatus::Completed => "completed",
        RunStatus::Failed => "failed",
        RunStatus::Cancelled => "cancelled",
    }
}

fn pending_tool_return_projection(
    tool_return: &ToolReturnPart,
    id_field: &str,
    id: &str,
) -> PyResult<Value> {
    let mut value = serde_json::to_value(tool_return).map_err(to_py_value_error)?;
    let Value::Object(object) = &mut value else {
        return Ok(value);
    };
    object.insert(id_field.to_string(), Value::String(id.to_string()));
    object
        .entry("tool_name".to_string())
        .or_insert_with(|| Value::String(tool_return.name.clone()));
    Ok(value)
}

fn stream_error_to_py(error: AgentStreamError) -> PyFutureError {
    match error {
        AgentStreamError::Agent(error) => PyFutureError::from_agent_error(error),
        AgentStreamError::RuntimeUnavailable(_)
        | AgentStreamError::Interrupted
        | AgentStreamError::Join(_) => PyFutureError::Stream(error.to_string()),
    }
}

fn hitl_error_to_py(error: AgentHitlError) -> PyFutureError {
    match error {
        AgentHitlError::Agent(error) => PyFutureError::from_agent_error(error),
        AgentHitlError::NoWaitingRun
        | AgentHitlError::NotWaiting { .. }
        | AgentHitlError::NoPendingHitl
        | AgentHitlError::UnknownApproval(_)
        | AgentHitlError::UnknownDeferred(_)
        | AgentHitlError::DuplicateDecision(_)
        | AgentHitlError::MissingDecisions { .. }
        | AgentHitlError::MissingToolCall(_)
        | AgentHitlError::DeferredResultNotTerminal { .. } => {
            PyFutureError::State(error.to_string())
        }
    }
}

fn stream_status_json(handle: &AgentStreamHandle) -> Value {
    let status = handle.status();
    serde_json::json!({
        "run_status": format!("{:?}", status.run_status).to_ascii_lowercase(),
        "current_error": status.current_error.map(|error| format!("{error:?}")),
        "cancel_requested": status.cancel_requested,
        "dropped_events": status.dropped_events,
        "receiver_closed": status.receiver_closed,
        "buffer_size": status.options.buffer_size,
        "drop_policy": format!("{:?}", status.options.drop_policy).to_ascii_lowercase(),
    })
}
