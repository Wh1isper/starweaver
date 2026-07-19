//! Python wrappers for Starweaver agents, runs, streams, and sessions.

use std::sync::{
    Arc, Mutex as StdMutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::{SystemTime, UNIX_EPOCH};

use pyo3::{
    exceptions::{PyRuntimeError, PyValueError},
    prelude::*,
};
use serde_json::{Map, Value};
use starweaver_agent::{
    AgentBuilder, AgentControlError, AgentControlHandle, AgentControlReceipt, AgentDurabilityError,
    AgentHitlError, AgentHitlResults, AgentLiveStreamResult, AgentRunOptions,
    AgentRuntime as SdkAgentRuntime, AgentRuntimeBuilder, AgentSession, AgentStreamController,
    AgentStreamDropPolicy, AgentStreamError, AgentStreamHandle, AgentStreamOptions, BusMessage,
    ModelConfig, ResumableState, SecurityConfig, ToolConfig,
};
use starweaver_core::{RunId, SessionId};
use starweaver_model::ToolReturnPart;
use starweaver_runtime::{
    Agent as RuntimeAgent, AgentInput, AgentResult, AgentRunState, AgentStreamResult, RunStatus,
};
use starweaver_session::{DeferredToolResults, ToolApprovalDecision};
use starweaver_tools::ToolRegistry;
use tokio::sync::Mutex;

use crate::{
    capability::PyCapabilityBundle,
    conversion::{json_to_py, optional_py_to_metadata, py_to_json, serialize_to_py},
    environment::extract_environment_provider,
    media::extract_media_uploader,
    model::{extract_model_settings, extract_request_params},
    output::{extract_output_policy, extract_output_schema},
    runtime::{PyFutureError, enter_runtime, spawn_py_future},
    skills::extract_skill_registry,
    store::{extract_replay_event_log, extract_session_store, extract_stream_archive},
    stream::PyStreamEvent,
    subagent::{PySubagent, parse_delegation_mode},
    testing::py_model_from_any,
    tool::PyPythonTool,
    toolset::{PyToolset, py_toolsets_to_dyn_toolsets},
};

/// Python wrapper around the Starweaver runtime agent.
#[pyclass(name = "Agent", skip_from_py_object)]
#[derive(Clone)]
pub struct PyAgent {
    inner: RuntimeAgent,
    default_environment: Option<starweaver_environment::DynEnvironmentProvider>,
    default_security: Option<SecurityConfig>,
}

impl PyAgent {
    pub(crate) fn runtime_agent(&self) -> RuntimeAgent {
        self.inner.clone()
    }
}

#[pymethods]
impl PyAgent {
    #[new]
    #[pyo3(signature = (model, tools=None, instructions=None, name=None, model_settings=None, request_params=None, output_schema=None, output_policy=None, subagents=None, subagent_delegation_mode=None, capability_bundles=None, toolsets=None, approval_required_tools=None, runtime_config=None, skills=None, environment=None, media_uploader=None, tool_config=None, security=None))]
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
        toolsets: Option<Vec<Py<PyToolset>>>,
        approval_required_tools: Option<Vec<String>>,
        runtime_config: Option<&Bound<'_, PyAny>>,
        skills: Option<&Bound<'_, PyAny>>,
        environment: Option<&Bound<'_, PyAny>>,
        media_uploader: Option<&Bound<'_, PyAny>>,
        tool_config: Option<&Bound<'_, PyAny>>,
        security: Option<&Bound<'_, PyAny>>,
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
        for toolset in py_toolsets_to_dyn_toolsets(py, toolsets)? {
            builder = builder.toolset(&toolset);
        }
        if let Some(approval_required_tools) = approval_required_tools {
            builder = builder.approval_required_tools(approval_required_tools);
        }
        if let Some(registry) = extract_skill_registry(py, skills)? {
            builder = builder.skills(registry);
        }
        if let Some(settings) = extract_model_settings(py, model_settings)? {
            builder = builder.model_settings(settings);
        }
        if let Some(model_config) = extract_runtime_model_config(py, runtime_config)? {
            builder = builder.model_config(model_config);
        }
        if let Some(tool_config) = extract_effective_tool_config(py, runtime_config, tool_config)? {
            builder = builder.tool_config(tool_config);
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
        if let Some(uploader) = extract_media_uploader(py, media_uploader)? {
            builder = builder.media_uploader(uploader);
        }
        let default_environment = extract_environment_provider(py, environment)?;
        let default_security = extract_effective_security_config(py, runtime_config, security)?;
        Ok(Self {
            inner: builder.build(),
            default_environment,
            default_security,
        })
    }

    fn run(&self, py: Python<'_>, prompt: String) -> PyResult<Py<PyAny>> {
        let agent = self.inner.clone();
        let environment = self.default_environment.clone();
        let security = self.default_security.clone();
        spawn_py_future(
            py,
            async move {
                if environment.is_some() || security.is_some() {
                    let mut session = AgentSession::new(agent);
                    if let Some(environment) = environment {
                        session.set_environment(environment);
                    }
                    if let Some(security) = security {
                        session.context_mut().security = security;
                    }
                    session
                        .run(prompt)
                        .await
                        .map_err(PyFutureError::from_agent_error)
                } else {
                    agent
                        .run(prompt)
                        .await
                        .map_err(PyFutureError::from_agent_error)
                }
            },
            |py, result| Ok(Py::new(py, PyRunResult::from_agent_result(result)?)?.into_any()),
        )
    }

    #[pyo3(signature = (prompt, instructions=None, tools=None, replace_tools=false, model_settings=None, request_params=None, output_schema=None, output_policy=None, trace_metadata=None, toolsets=None, environment=None, context_metadata=None, tool_config=None, security=None))]
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
        trace_metadata: Option<&Bound<'_, PyAny>>,
        toolsets: Option<Vec<Py<PyToolset>>>,
        environment: Option<&Bound<'_, PyAny>>,
        context_metadata: Option<&Bound<'_, PyAny>>,
        tool_config: Option<&Bound<'_, PyAny>>,
        security: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<PyAgentStream> {
        let mut session = AgentSession::new(self.inner.clone());
        if let Some(environment) = extract_environment_provider(py, environment)?
            .or_else(|| self.default_environment.clone())
        {
            session.set_environment(environment);
        }
        if let Some(security) = self.default_security.clone() {
            session.context_mut().security = security;
        }
        let options = py_run_options(
            py,
            instructions,
            tools,
            replace_tools,
            model_settings,
            request_params,
            output_schema,
            output_policy,
            trace_metadata,
            context_metadata,
            tool_config,
            security,
            toolsets,
        )?;
        let handle =
            enter_runtime(|| {
                Ok(session.stream_with_run_and_stream_options(
                    prompt,
                    options,
                    python_stream_options(),
                ))
            })?;
        Ok(PyAgentStream::new(
            handle,
            Some(Arc::new(Mutex::new(session))),
            None,
        ))
    }

    #[pyo3(signature = (environment=None))]
    fn new_session(
        &self,
        py: Python<'_>,
        environment: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<PySession> {
        let mut session = AgentSession::new(self.inner.clone());
        if let Some(environment) = extract_environment_provider(py, environment)?
            .or_else(|| self.default_environment.clone())
        {
            session.set_environment(environment);
        }
        if let Some(security) = self.default_security.clone() {
            session.context_mut().security = security;
        }
        Ok(PySession {
            inner: Arc::new(Mutex::new(session)),
            busy: Arc::new(AtomicBool::new(false)),
            active_control: Arc::new(StdMutex::new(None)),
            active_control_seq: Arc::new(AtomicUsize::new(0)),
        })
    }

    #[pyo3(signature = (state, environment=None))]
    fn session_from_state(
        &self,
        py: Python<'_>,
        state: &Bound<'_, PyAny>,
        environment: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<PySession> {
        let state: ResumableState = serde_json::from_value(py_to_json(py, state)?)
            .map_err(|error| PyValueError::new_err(format!("invalid session state: {error}")))?;
        let mut session = AgentSession::from_state(self.inner.clone(), state);
        if let Some(environment) = extract_environment_provider(py, environment)?
            .or_else(|| self.default_environment.clone())
        {
            session.set_environment(environment);
        }
        if let Some(security) = self.default_security.clone() {
            session.context_mut().security = security;
        }
        Ok(PySession {
            inner: Arc::new(Mutex::new(session)),
            busy: Arc::new(AtomicBool::new(false)),
            active_control: Arc::new(StdMutex::new(None)),
            active_control_seq: Arc::new(AtomicUsize::new(0)),
        })
    }
}

/// Python wrapper around the owned Starweaver SDK runtime.
#[pyclass(name = "AgentRuntime", skip_from_py_object)]
#[derive(Clone)]
pub struct PyAgentRuntime {
    inner: Arc<Mutex<SdkAgentRuntime>>,
}

#[pymethods]
impl PyAgentRuntime {
    #[new]
    #[pyo3(signature = (model, tools=None, instructions=None, name=None, model_settings=None, request_params=None, output_schema=None, output_policy=None, subagents=None, subagent_delegation_mode=None, capability_bundles=None, toolsets=None, approval_required_tools=None, runtime_config=None, skills=None, environment=None, media_uploader=None, session_store=None, durable_session_id=None, stream_archive=None, replay_event_log=None, state=None, tool_config=None, security=None))]
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
        toolsets: Option<Vec<Py<PyToolset>>>,
        approval_required_tools: Option<Vec<String>>,
        runtime_config: Option<&Bound<'_, PyAny>>,
        skills: Option<&Bound<'_, PyAny>>,
        environment: Option<&Bound<'_, PyAny>>,
        media_uploader: Option<&Bound<'_, PyAny>>,
        session_store: Option<&Bound<'_, PyAny>>,
        durable_session_id: Option<String>,
        stream_archive: Option<&Bound<'_, PyAny>>,
        replay_event_log: Option<&Bound<'_, PyAny>>,
        state: Option<&Bound<'_, PyAny>>,
        tool_config: Option<&Bound<'_, PyAny>>,
        security: Option<&Bound<'_, PyAny>>,
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
        for toolset in py_toolsets_to_dyn_toolsets(py, toolsets)? {
            builder = builder.toolset(&toolset);
        }
        if let Some(approval_required_tools) = approval_required_tools {
            builder = builder.approval_required_tools(approval_required_tools);
        }
        if let Some(registry) = extract_skill_registry(py, skills)? {
            builder = builder.skills(registry);
        }
        if let Some(settings) = extract_model_settings(py, model_settings)? {
            builder = builder.model_settings(settings);
        }
        if let Some(model_config) = extract_runtime_model_config(py, runtime_config)? {
            builder = builder.model_config(model_config);
        }
        if let Some(tool_config) = extract_effective_tool_config(py, runtime_config, tool_config)? {
            builder = builder.tool_config(tool_config);
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
        if let Some(uploader) = extract_media_uploader(py, media_uploader)? {
            builder = builder.media_uploader(uploader);
        }
        let mut builder = AgentRuntimeBuilder::from_builder(builder);
        if let Some(store) = extract_session_store(session_store)? {
            builder = builder.session_store(store);
        }
        if let Some(session_id) = durable_session_id {
            builder = builder.durable_session_id(SessionId::from_string(session_id));
        }
        if let Some(archive) = extract_stream_archive(stream_archive)? {
            builder = builder.stream_archive(archive);
        }
        if let Some(log) = extract_replay_event_log(replay_event_log)? {
            builder = builder.replay_event_log(log);
        }
        if let Some(state) = state {
            let state: ResumableState =
                serde_json::from_value(py_to_json(py, state)?).map_err(|error| {
                    PyValueError::new_err(format!("invalid runtime state: {error}"))
                })?;
            builder = builder.state(state);
        }
        if let Some(environment) = extract_environment_provider(py, environment)? {
            builder = builder.environment(environment);
        }
        if let Some(security) = extract_effective_security_config(py, runtime_config, security)? {
            builder = builder.security(security);
        }
        Ok(Self {
            inner: Arc::new(Mutex::new(builder.build())),
        })
    }

    #[getter]
    fn durable_session_id(&self) -> Option<String> {
        self.inner
            .blocking_lock()
            .durable_session_id()
            .map(|session_id| session_id.as_str().to_string())
    }

    fn run(&self, py: Python<'_>, prompt: String) -> PyResult<Py<PyAny>> {
        let runtime = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                let mut runtime = runtime.lock().await;
                runtime
                    .run(prompt)
                    .await
                    .map_err(PyFutureError::from_agent_error)
            },
            |py, result| Ok(Py::new(py, PyRunResult::from_agent_result(result)?)?.into_any()),
        )
    }

    fn run_stream(&self, py: Python<'_>, prompt: String) -> PyResult<Py<PyAny>> {
        let runtime = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                let mut runtime = runtime.lock().await;
                runtime
                    .run_stream(prompt)
                    .await
                    .map_err(PyFutureError::from_agent_error)
            },
            |py, result| Ok(Py::new(py, PyStreamRunResult::from_stream_result(result)?)?.into_any()),
        )
    }

    fn stream(&self, prompt: String) -> PyResult<PyAgentStream> {
        let mut runtime = self
            .inner
            .try_lock()
            .map_err(|_| PyRuntimeError::new_err("runtime is busy"))?;
        let handle = enter_runtime(|| {
            Ok(runtime.stream_with_stream_options(prompt.clone(), python_stream_options()))
        })?;
        Ok(PyAgentStream::new_runtime(
            handle,
            self.inner.clone(),
            prompt,
        ))
    }

    fn export_state(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let runtime = self.inner.blocking_lock();
        serialize_to_py(py, &runtime.export_state())
    }

    fn export_full_state(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let runtime = self.inner.blocking_lock();
        serialize_to_py(py, &runtime.export_full_state())
    }

    fn export_environment_state(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let runtime = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                let runtime = runtime.lock().await;
                runtime
                    .export_environment_state()
                    .await
                    .map_err(|error| PyFutureError::State(error.to_string()))
            },
            |py, state| match state {
                Some(state) => serialize_to_py(py, &state),
                None => Ok(py.None()),
            },
        )
    }

    fn set_environment(&self, py: Python<'_>, environment: &Bound<'_, PyAny>) -> PyResult<()> {
        let environment = extract_environment_provider(py, Some(environment))?
            .ok_or_else(|| PyValueError::new_err("environment must not be None"))?;
        let mut runtime = self.inner.blocking_lock();
        runtime.restore_environment(environment);
        Ok(())
    }

    fn resume_snapshot(
        &self,
        py: Python<'_>,
        session_id: String,
        run_id: String,
    ) -> PyResult<Py<PyAny>> {
        let runtime = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                let runtime = runtime.lock().await;
                runtime
                    .resume_snapshot(
                        &SessionId::from_string(session_id),
                        &RunId::from_string(run_id),
                    )
                    .await
                    .map_err(durability_error_to_py)
            },
            |py, snapshot| serialize_to_py(py, &snapshot),
        )
    }

    #[pyo3(signature = (session_id, run_id, approvals=None, deferred_results=None))]
    fn resume_after_hitl_by_id(
        &self,
        py: Python<'_>,
        session_id: String,
        run_id: String,
        approvals: Option<&Bound<'_, PyAny>>,
        deferred_results: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        let runtime = self.inner.clone();
        let results = parse_hitl_results(py, approvals, deferred_results)?;
        spawn_py_future(
            py,
            async move {
                let mut runtime = runtime.lock().await;
                runtime
                    .resume_after_hitl_by_id(
                        &SessionId::from_string(session_id),
                        &RunId::from_string(run_id),
                        results,
                    )
                    .await
                    .map_err(durability_error_to_py)
            },
            |py, result| Ok(Py::new(py, PyRunResult::from_agent_result(result)?)?.into_any()),
        )
    }
}

/// Live Python stream handle over Starweaver stream records.
#[pyclass(name = "AgentStream", skip_from_py_object)]
#[derive(Clone)]
pub struct PyAgentStream {
    handle: Arc<Mutex<Option<AgentStreamHandle>>>,
    controller: AgentStreamController,
    session: Option<Arc<Mutex<AgentSession>>>,
    runtime: Option<Arc<Mutex<SdkAgentRuntime>>>,
    runtime_input: Option<String>,
    session_lease: Option<Arc<PySessionOperationLease>>,
    detached: Arc<AtomicBool>,
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
            runtime: None,
            runtime_input: None,
            session_lease,
            detached: Arc::new(AtomicBool::new(false)),
        }
    }

    fn new_runtime(
        handle: AgentStreamHandle,
        runtime: Arc<Mutex<SdkAgentRuntime>>,
        input: String,
    ) -> Self {
        let controller = handle.controller();
        Self {
            handle: Arc::new(Mutex::new(Some(handle))),
            controller,
            session: None,
            runtime: Some(runtime),
            runtime_input: Some(input),
            session_lease: None,
            detached: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl Drop for PyAgentStream {
    fn drop(&mut self) {
        if !self.detached.load(Ordering::SeqCst) {
            self.controller.interrupt();
            if let Some(lease) = &self.session_lease {
                lease.release();
            }
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

    fn close_receiver(&self) -> PyResult<()> {
        let mut guard = self
            .handle
            .try_lock()
            .map_err(|_| PyRuntimeError::new_err("stream is busy"))?;
        if let Some(handle) = guard.as_mut() {
            handle.close_receiver();
        }
        Ok(())
    }

    fn detach(&self) -> PyResult<()> {
        let handle = self.handle.clone();
        let session = self.session.clone();
        let runtime = self.runtime.clone();
        let runtime_input = self.runtime_input.clone();
        let session_lease = self.session_lease.clone();
        let detached = self.detached.clone();
        enter_runtime(|| {
            detached.store(true, Ordering::SeqCst);
            tokio::spawn(async move {
                let stream = {
                    let mut guard = handle.lock().await;
                    guard.take()
                };
                if let Some(mut stream) = stream {
                    stream.close_receiver();
                    if let Some(runtime) = runtime {
                        let mut runtime = runtime.lock().await;
                        let input = runtime_input.unwrap_or_default();
                        let _ = runtime.finish_stream(AgentInput::text(input), stream).await;
                    } else if let Some(session) = session {
                        let mut session = session.lock().await;
                        let _ = stream.finish_into_session(&mut session).await;
                    } else {
                        let _ = stream.join().await;
                    }
                }
                if let Some(lease) = &session_lease {
                    lease.release();
                }
            });
            Ok(())
        })
    }

    #[pyo3(signature = (reason=None))]
    fn interrupt(&self, reason: Option<String>) -> PyResult<()> {
        let _ = self.controller.control_handle().interrupt(reason);
        Ok(())
    }

    #[pyo3(signature = (text, id=None))]
    fn steer(&self, py: Python<'_>, text: String, id: Option<String>) -> PyResult<Py<PyAny>> {
        let control = self.controller.control_handle();
        let id = id.unwrap_or_else(|| generated_control_id("steering"));
        spawn_py_future(
            py,
            async move { control.steer(id, text).await.map_err(control_error_to_py) },
            control_receipt_to_py,
        )
    }

    fn send_message(&self, py: Python<'_>, message: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        let message = parse_bus_message(py, message)?;
        let control = self.controller.control_handle();
        spawn_py_future(
            py,
            async move {
                control
                    .send_message(message)
                    .await
                    .map_err(control_error_to_py)
            },
            control_receipt_to_py,
        )
    }

    fn join(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let handle = self.handle.clone();
        let session = self.session.clone();
        let runtime = self.runtime.clone();
        let runtime_input = self.runtime_input.clone();
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
                    let result = if let Some(runtime) = runtime {
                        let mut runtime = runtime.lock().await;
                        let input = runtime_input.unwrap_or_default();
                        runtime
                            .finish_stream(AgentInput::text(input), stream)
                            .await
                            .map_err(durability_error_to_py)?
                    } else if let Some(session) = session {
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
        let runtime = self.runtime.clone();
        let runtime_input = self.runtime_input.clone();
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
                    let result = if let Some(runtime) = runtime {
                        let mut runtime = runtime.lock().await;
                        let input = runtime_input.unwrap_or_default();
                        runtime
                            .finish_stream(AgentInput::text(input), stream)
                            .await
                            .map_err(durability_error_to_py)?
                    } else if let Some(session) = session {
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

    #[pyo3(signature = (approvals=None, deferred_results=None))]
    fn resume_after_hitl(
        &self,
        py: Python<'_>,
        approvals: Option<&Bound<'_, PyAny>>,
        deferred_results: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        let Some(session) = self.session.clone() else {
            return Err(PyRuntimeError::new_err(
                "stream is not bound to a resumable session",
            ));
        };
        let results = parse_hitl_results(py, approvals, deferred_results)?;
        spawn_py_future(
            py,
            async move {
                let mut session = session.lock().await;
                session
                    .resume_after_hitl(results)
                    .await
                    .map_err(hitl_error_to_py)
            },
            |py, result| Ok(Py::new(py, PyRunResult::from_agent_result(result)?)?.into_any()),
        )
    }

    #[pyo3(signature = (approvals=None, deferred_results=None))]
    fn resume_after_hitl_stream(
        &self,
        py: Python<'_>,
        approvals: Option<&Bound<'_, PyAny>>,
        deferred_results: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        let Some(session) = self.session.clone() else {
            return Err(PyRuntimeError::new_err(
                "stream is not bound to a resumable session",
            ));
        };
        let results = parse_hitl_results(py, approvals, deferred_results)?;
        spawn_py_future(
            py,
            async move {
                let handle = {
                    let mut session_guard = session.lock().await;
                    if results.is_empty() {
                        if session_guard.context().pending_tool_returns.is_empty() {
                            return Err(hitl_error_to_py(AgentHitlError::NoWaitingRun));
                        }
                    } else {
                        session_guard
                            .inject_hitl_results(results)
                            .await
                            .map_err(hitl_error_to_py)?;
                    }
                    session_guard.stream_with_run_and_stream_options(
                        "",
                        AgentRunOptions::new(),
                        python_stream_options(),
                    )
                };
                Ok((handle, session))
            },
            |py, (handle, session)| {
                Ok(Py::new(py, PyAgentStream::new(handle, Some(session), None))?.into_any())
            },
        )
    }

    #[pyo3(signature = (state, approvals=None, deferred_results=None))]
    fn resume_after_hitl_for_state(
        &self,
        py: Python<'_>,
        state: &Bound<'_, PyAny>,
        approvals: Option<&Bound<'_, PyAny>>,
        deferred_results: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        let Some(session) = self.session.clone() else {
            return Err(PyRuntimeError::new_err(
                "stream is not bound to a resumable session",
            ));
        };
        let state: AgentRunState = serde_json::from_value(py_to_json(py, state)?)
            .map_err(|error| PyValueError::new_err(format!("invalid run state: {error}")))?;
        let results = parse_hitl_results(py, approvals, deferred_results)?;
        spawn_py_future(
            py,
            async move {
                let mut session = session.lock().await;
                session
                    .resume_after_hitl_for_state(&state, results)
                    .await
                    .map_err(hitl_error_to_py)
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
                    "live_state": "closed",
                    "is_terminal": true,
                    "current_error": null,
                    "cancel_requested": self.controller.cancel_requested(),
                    "dropped_events": 0,
                    "receiver_closed": true,
                    "options": python_stream_options(),
                    "buffer_size": python_stream_options().buffer_size,
                    "drop_policy": python_stream_options().drop_policy,
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
    active_control: Arc<StdMutex<Option<PyActiveControl>>>,
    active_control_seq: Arc<AtomicUsize>,
}

impl PySession {
    fn acquire_operation(&self) -> PyResult<Arc<PySessionOperationLease>> {
        PySessionOperationLease::acquire(self.busy.clone(), None)
    }

    fn acquire_stream_operation(
        &self,
        active_control_token: Option<usize>,
    ) -> PyResult<Arc<PySessionOperationLease>> {
        PySessionOperationLease::acquire(
            self.busy.clone(),
            active_control_token.map(|token| (self.active_control.clone(), token)),
        )
    }

    fn next_active_control_token(&self) -> usize {
        self.active_control_seq.fetch_add(1, Ordering::SeqCst) + 1
    }
}

#[derive(Clone)]
struct PyActiveControl {
    token: usize,
    handle: AgentControlHandle,
}

struct PySessionOperationLease {
    busy: Arc<AtomicBool>,
    active_control: Option<(Arc<StdMutex<Option<PyActiveControl>>>, usize)>,
    released: AtomicBool,
}

impl PySessionOperationLease {
    fn acquire(
        busy: Arc<AtomicBool>,
        active_control: Option<(Arc<StdMutex<Option<PyActiveControl>>>, usize)>,
    ) -> PyResult<Arc<Self>> {
        if busy
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err(PyRuntimeError::new_err("session is busy"));
        }
        Ok(Arc::new(Self {
            busy,
            active_control,
            released: AtomicBool::new(false),
        }))
    }

    fn release(&self) {
        if !self.released.swap(true, Ordering::SeqCst) {
            if let Some((active_control, token)) = &self.active_control
                && let Ok(mut guard) = active_control.lock()
                && guard.as_ref().is_some_and(|active| active.token == *token)
            {
                *guard = None;
            }
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
        let lease = self.acquire_stream_operation(None)?;
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

    #[pyo3(signature = (prompt, instructions=None, tools=None, replace_tools=false, model_settings=None, request_params=None, output_schema=None, output_policy=None, trace_metadata=None, toolsets=None, environment=None, context_metadata=None, tool_config=None, security=None))]
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
        trace_metadata: Option<&Bound<'_, PyAny>>,
        toolsets: Option<Vec<Py<PyToolset>>>,
        environment: Option<&Bound<'_, PyAny>>,
        context_metadata: Option<&Bound<'_, PyAny>>,
        tool_config: Option<&Bound<'_, PyAny>>,
        security: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<PyAgentStream> {
        let active_control_token = self.next_active_control_token();
        let lease = self.acquire_stream_operation(Some(active_control_token))?;
        let options = py_run_options(
            py,
            instructions,
            tools,
            replace_tools,
            model_settings,
            request_params,
            output_schema,
            output_policy,
            trace_metadata,
            context_metadata,
            tool_config,
            security,
            toolsets,
        )?;
        let mut session = self
            .inner
            .try_lock()
            .map_err(|_| PyRuntimeError::new_err("session is busy"))?;
        if let Some(environment) = extract_environment_provider(py, environment)? {
            session.set_environment(environment);
        }
        let handle =
            enter_runtime(|| {
                Ok(session.stream_with_run_and_stream_options(
                    prompt,
                    options,
                    python_stream_options(),
                ))
            })?;
        let control = handle.control_handle();
        {
            let mut active_control = self
                .active_control
                .lock()
                .map_err(|_| PyRuntimeError::new_err("session active control lock poisoned"))?;
            *active_control = Some(PyActiveControl {
                token: active_control_token,
                handle: control,
            });
        }
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

    #[getter]
    fn metadata(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let session = self.inner.blocking_lock();
        json_to_py(
            py,
            &serde_json::Value::Object(session.context().metadata.clone()),
        )
    }

    fn set_metadata(&self, py: Python<'_>, key: String, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let value = py_to_json(py, value)?;
        let mut session = self
            .inner
            .try_lock()
            .map_err(|_| PyRuntimeError::new_err("session is busy"))?;
        session.set_metadata(key, value);
        Ok(())
    }

    fn set_environment(&self, py: Python<'_>, environment: &Bound<'_, PyAny>) -> PyResult<()> {
        let _lease = self.acquire_operation()?;
        let environment = extract_environment_provider(py, Some(environment))?
            .ok_or_else(|| PyValueError::new_err("environment must not be None"))?;
        let mut session = self.inner.blocking_lock();
        session.set_environment(environment);
        Ok(())
    }

    fn export_environment_state(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let _lease = self.acquire_operation()?;
        let session = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                let session = session.lock().await;
                session
                    .export_environment_state()
                    .await
                    .map_err(|error| PyFutureError::State(error.to_string()))
            },
            |py, state| match state {
                Some(state) => serialize_to_py(py, &state),
                None => Ok(py.None()),
            },
        )
    }

    #[pyo3(signature = (text, id=None))]
    fn steer(&self, py: Python<'_>, text: String, id: Option<String>) -> PyResult<Py<PyAny>> {
        let control = self
            .active_control
            .lock()
            .map_err(|_| PyRuntimeError::new_err("session active control lock poisoned"))?
            .as_ref()
            .map(|active| active.handle.clone())
            .ok_or_else(|| {
                state_error_with_code(py, "no active run for session", "no_active_run")
            })?;
        let id = id.unwrap_or_else(|| generated_control_id("steering"));
        spawn_py_future(
            py,
            async move { control.steer(id, text).await.map_err(control_error_to_py) },
            control_receipt_to_py,
        )
    }

    #[pyo3(signature = (reason=None))]
    fn interrupt(&self, py: Python<'_>, reason: Option<String>) -> PyResult<()> {
        let control = self
            .active_control
            .lock()
            .map_err(|_| PyRuntimeError::new_err("session active control lock poisoned"))?
            .as_ref()
            .map(|active| active.handle.clone())
            .ok_or_else(|| {
                state_error_with_code(py, "no active run for session", "no_active_run")
            })?;
        let _ = control.interrupt(reason);
        Ok(())
    }

    fn message_send(&self, py: Python<'_>, message: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        let _lease = self.acquire_operation()?;
        let message = parse_bus_message(py, message)?;
        let mut session = self.inner.blocking_lock();
        let sent = session.context_mut().send_message(message);
        serialize_to_py(py, &sent)
    }

    #[pyo3(signature = (agent_id=None))]
    fn message_peek(&self, py: Python<'_>, agent_id: Option<String>) -> PyResult<Py<PyAny>> {
        let _lease = self.acquire_operation()?;
        let session = self.inner.blocking_lock();
        let agent_id = agent_id.unwrap_or_else(|| session.context().agent_id.as_str().to_string());
        serialize_to_py(py, &session.context().messages.peek(&agent_id))
    }

    #[pyo3(signature = (agent_id=None))]
    fn message_consume(&self, py: Python<'_>, agent_id: Option<String>) -> PyResult<Py<PyAny>> {
        let _lease = self.acquire_operation()?;
        let mut session = self.inner.blocking_lock();
        let agent_id = agent_id.unwrap_or_else(|| session.context().agent_id.as_str().to_string());
        let messages = session.context_mut().consume_messages_for(&agent_id);
        serialize_to_py(py, &messages)
    }

    #[pyo3(signature = (agent_id=None))]
    fn message_subscribe(&self, agent_id: Option<String>) -> PyResult<()> {
        let _lease = self.acquire_operation()?;
        let mut session = self.inner.blocking_lock();
        let agent_id = agent_id.unwrap_or_else(|| session.context().agent_id.as_str().to_string());
        session.context_mut().messages.subscribe(agent_id);
        Ok(())
    }

    #[pyo3(signature = (agent_id=None))]
    fn message_unsubscribe(&self, agent_id: Option<String>) -> PyResult<()> {
        let _lease = self.acquire_operation()?;
        let mut session = self.inner.blocking_lock();
        let agent_id = agent_id.unwrap_or_else(|| session.context().agent_id.as_str().to_string());
        session.context_mut().messages.unsubscribe(&agent_id);
        Ok(())
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

    #[pyo3(signature = (approvals=None, deferred_results=None))]
    fn resume_after_hitl_stream(
        &self,
        py: Python<'_>,
        approvals: Option<&Bound<'_, PyAny>>,
        deferred_results: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        let active_control_token = self.next_active_control_token();
        let lease = self.acquire_stream_operation(Some(active_control_token))?;
        let results = parse_hitl_results(py, approvals, deferred_results)?;
        let session = self.inner.clone();
        let active_control = self.active_control.clone();
        spawn_py_future(
            py,
            async move {
                let handle = {
                    let mut session_guard = session.lock().await;
                    if results.is_empty() {
                        if session_guard.context().pending_tool_returns.is_empty() {
                            return Err(hitl_error_to_py(AgentHitlError::NoWaitingRun));
                        }
                    } else {
                        session_guard
                            .inject_hitl_results(results)
                            .await
                            .map_err(hitl_error_to_py)?;
                    }
                    session_guard.stream_with_run_and_stream_options(
                        "",
                        AgentRunOptions::new(),
                        python_stream_options(),
                    )
                };
                let control = handle.control_handle();
                {
                    let mut guard = active_control.lock().map_err(|_| {
                        PyFutureError::State("session active control lock poisoned".to_string())
                    })?;
                    *guard = Some(PyActiveControl {
                        token: active_control_token,
                        handle: control,
                    });
                }
                Ok((handle, session, lease))
            },
            |py, (handle, session, lease)| {
                Ok(Py::new(py, PyAgentStream::new(handle, Some(session), Some(lease)))?.into_any())
            },
        )
    }

    #[pyo3(signature = (state, approvals=None, deferred_results=None))]
    fn resume_after_hitl_for_state(
        &self,
        py: Python<'_>,
        state: &Bound<'_, PyAny>,
        approvals: Option<&Bound<'_, PyAny>>,
        deferred_results: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        let lease = self.acquire_operation()?;
        let state: AgentRunState = serde_json::from_value(py_to_json(py, state)?)
            .map_err(|error| PyValueError::new_err(format!("invalid run state: {error}")))?;
        let results = parse_hitl_results(py, approvals, deferred_results)?;
        let session = self.inner.clone();
        spawn_py_future(
            py,
            async move {
                let _lease = lease;
                let mut session = session.lock().await;
                session
                    .resume_after_hitl_for_state(&state, results)
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
            let Some(approved) = object.remove("approved").and_then(|value| value.as_bool()) else {
                return Err(PyValueError::new_err(
                    "approval decision mappings must include approved: bool",
                ));
            };
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

fn parse_bus_message(py: Python<'_>, message: &Bound<'_, PyAny>) -> PyResult<BusMessage> {
    serde_json::from_value(py_to_json(py, message)?)
        .map_err(|error| PyValueError::new_err(format!("invalid bus message: {error}")))
}

fn control_receipt_to_py(py: Python<'_>, receipt: AgentControlReceipt) -> PyResult<Py<PyAny>> {
    serialize_to_py(py, &receipt.snapshot())
}

fn control_error_to_py(error: AgentControlError) -> PyFutureError {
    PyFutureError::StateWithCode {
        code: error.code_str(),
        message: error.to_string(),
    }
}

fn state_error_with_code(py: Python<'_>, message: &str, code: &'static str) -> PyErr {
    match py
        .import("starweaver.errors")
        .and_then(|module| module.getattr("StateError"))
        .and_then(|error_class| {
            error_class
                .call1((message,))?
                .call_method1("with_code", (code,))
        }) {
        Ok(error) => PyErr::from_value(error),
        Err(_) => PyRuntimeError::new_err(message.to_string()),
    }
}

fn generated_control_id(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    format!("{prefix}-{nanos}")
}

fn python_stream_options() -> AgentStreamOptions {
    AgentStreamOptions::new().drop_policy(AgentStreamDropPolicy::Backpressure)
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
    trace_metadata: Option<&Bound<'_, PyAny>>,
    context_metadata: Option<&Bound<'_, PyAny>>,
    tool_config: Option<&Bound<'_, PyAny>>,
    security: Option<&Bound<'_, PyAny>>,
    toolsets: Option<Vec<Py<PyToolset>>>,
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
    let native_toolsets = py_toolsets_to_dyn_toolsets(py, toolsets)?;
    if !native_toolsets.is_empty() {
        options = options.toolsets(native_toolsets);
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
    let metadata = optional_py_to_metadata(py, trace_metadata)?;
    if !metadata.is_empty() {
        options = options.trace_metadata(metadata);
    }
    let metadata = optional_py_to_metadata(py, context_metadata)?;
    if !metadata.is_empty() {
        options = options.context_metadata(metadata);
    }
    if let Some(tool_config) = extract_tool_config(py, tool_config)? {
        options = options.tool_config(tool_config);
    }
    if let Some(security) = extract_security_config(py, security)? {
        options = options.security(security);
    }
    Ok(options)
}

fn extract_runtime_model_config(
    py: Python<'_>,
    runtime_config: Option<&Bound<'_, PyAny>>,
) -> PyResult<Option<ModelConfig>> {
    let Some(value) = runtime_config_json(py, runtime_config)? else {
        return Ok(None);
    };
    let model_config_value = value.get("model_config").cloned().unwrap_or(value);
    serde_json::from_value(model_config_value)
        .map(Some)
        .map_err(|error| PyValueError::new_err(format!("invalid runtime_config: {error}")))
}

fn extract_effective_tool_config(
    py: Python<'_>,
    runtime_config: Option<&Bound<'_, PyAny>>,
    tool_config: Option<&Bound<'_, PyAny>>,
) -> PyResult<Option<ToolConfig>> {
    if let Some(tool_config) = extract_tool_config(py, tool_config)? {
        return Ok(Some(tool_config));
    }
    let Some(value) = runtime_config_json(py, runtime_config)? else {
        return Ok(None);
    };
    let Some(tool_config_value) = value.get("tool_config").cloned() else {
        return Ok(None);
    };
    serde_json::from_value(tool_config_value)
        .map(Some)
        .map_err(|error| {
            PyValueError::new_err(format!("invalid runtime_config.tool_config: {error}"))
        })
}

fn extract_effective_security_config(
    py: Python<'_>,
    runtime_config: Option<&Bound<'_, PyAny>>,
    security: Option<&Bound<'_, PyAny>>,
) -> PyResult<Option<SecurityConfig>> {
    if let Some(security) = extract_security_config(py, security)? {
        return Ok(Some(security));
    }
    let Some(value) = runtime_config_json(py, runtime_config)? else {
        return Ok(None);
    };
    let Some(security_value) = value.get("security").cloned() else {
        return Ok(None);
    };
    serde_json::from_value(security_value)
        .map(Some)
        .map_err(|error| PyValueError::new_err(format!("invalid runtime_config.security: {error}")))
}

fn extract_tool_config(
    py: Python<'_>,
    tool_config: Option<&Bound<'_, PyAny>>,
) -> PyResult<Option<ToolConfig>> {
    let Some(value) = config_json(py, tool_config)? else {
        return Ok(None);
    };
    let tool_config_value = value.get("tool_config").cloned().unwrap_or(value);
    serde_json::from_value(tool_config_value)
        .map(Some)
        .map_err(|error| PyValueError::new_err(format!("invalid tool_config: {error}")))
}

fn extract_security_config(
    py: Python<'_>,
    security: Option<&Bound<'_, PyAny>>,
) -> PyResult<Option<SecurityConfig>> {
    let Some(value) = config_json(py, security)? else {
        return Ok(None);
    };
    let security_value = value.get("security").cloned().unwrap_or(value);
    serde_json::from_value(security_value)
        .map(Some)
        .map_err(|error| PyValueError::new_err(format!("invalid security: {error}")))
}

fn runtime_config_json(
    py: Python<'_>,
    runtime_config: Option<&Bound<'_, PyAny>>,
) -> PyResult<Option<Value>> {
    config_json(py, runtime_config)
}

fn config_json(py: Python<'_>, value: Option<&Bound<'_, PyAny>>) -> PyResult<Option<Value>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_none() {
        return Ok(None);
    }
    match value.getattr("to_dict") {
        Ok(to_dict) => py_to_json(py, &to_dict.call0()?).map(Some),
        Err(_) => py_to_json(py, value).map(Some),
    }
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
        | AgentStreamError::Interrupted { .. }
        | AgentStreamError::Join(_) => PyFutureError::Stream(error.to_string()),
    }
}

fn hitl_error_to_py(error: AgentHitlError) -> PyFutureError {
    match error {
        AgentHitlError::Agent(error) => PyFutureError::from_agent_error(error),
        error => PyFutureError::StateWithCode {
            code: error.code_str(),
            message: error.to_string(),
        },
    }
}

fn durability_error_to_py(error: AgentDurabilityError) -> PyFutureError {
    match error {
        AgentDurabilityError::Agent(error) => PyFutureError::from_agent_error(error),
        AgentDurabilityError::Stream(error) => stream_error_to_py(error),
        AgentDurabilityError::Hitl(error) => hitl_error_to_py(error),
        AgentDurabilityError::MissingSessionStore
        | AgentDurabilityError::MissingCheckpointState { .. }
        | AgentDurabilityError::SessionMismatch { .. }
        | AgentDurabilityError::InvalidContinuationEvidence(_)
        | AgentDurabilityError::MissingPublicationSink { .. }
        | AgentDurabilityError::SessionStore(_)
        | AgentDurabilityError::Replay(_) => PyFutureError::State(error.to_string()),
    }
}

fn stream_status_json(handle: &AgentStreamHandle) -> Value {
    serde_json::to_value(handle.status().snapshot())
        .expect("live stream status snapshot should be JSON serializable")
}
