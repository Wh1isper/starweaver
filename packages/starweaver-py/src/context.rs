//! Python context projections.

use pyo3::{prelude::*, types::PyBool};
use serde_json::Value;
use starweaver_agent::EnvironmentHandle;
use starweaver_context::{AgentContext, AgentContextHandle};
use starweaver_core::Metadata;
use starweaver_environment::{DynEnvironmentProvider, EnvironmentState};
use starweaver_tools::ToolContext;

use crate::{
    conversion::{json_to_py, py_to_json, serialize_to_py},
    environment::PyEnvironmentProvider,
    runtime::{PyFutureError, spawn_py_future},
};

/// Python projection of a shared Starweaver context snapshot handle.
#[pyclass(name = "AgentContextHandle", skip_from_py_object)]
#[derive(Clone)]
pub struct PyAgentContextHandle {
    inner: AgentContextHandle,
}

impl PyAgentContextHandle {
    const fn new(inner: AgentContextHandle) -> Self {
        Self { inner }
    }
}

#[pymethods]
impl PyAgentContextHandle {
    fn snapshot(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        serialize_to_py(py, &self.inner.snapshot().export_full_state())
    }

    #[getter]
    fn metadata(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        json_to_py(
            py,
            &serde_json::Value::Object(self.inner.snapshot().metadata.clone()),
        )
    }

    fn set_metadata(&self, py: Python<'_>, key: String, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let value = py_to_json(py, value)?;
        self.inner.update(|context| {
            context.metadata.insert(key, value);
        });
        Ok(())
    }
}

/// Python projection of Starweaver's tool execution context.
#[pyclass(name = "ToolContext", skip_from_py_object)]
#[derive(Clone)]
pub struct PyToolContext {
    inner: ToolContext,
}

impl PyToolContext {
    pub(crate) const fn new(inner: ToolContext) -> Self {
        Self { inner }
    }

    fn context_snapshot(&self) -> Option<AgentContext> {
        self.inner
            .dependency::<AgentContextHandle>()
            .map(|handle| handle.snapshot())
            .or_else(|| {
                self.inner
                    .dependency::<AgentContext>()
                    .map(|context| context.as_ref().clone())
            })
    }

    fn context_handle_snapshot(&self) -> Option<AgentContextHandle> {
        self.inner
            .dependency::<AgentContextHandle>()
            .map(|handle| handle.as_ref().clone())
    }

    fn environment_provider(&self) -> Option<DynEnvironmentProvider> {
        self.inner
            .dependency::<EnvironmentHandle>()
            .map(|handle| handle.provider())
            .or_else(|| {
                self.context_snapshot().and_then(|context| {
                    context
                        .dependencies
                        .get::<EnvironmentHandle>()
                        .map(|handle| handle.provider())
                })
            })
    }

    fn public_metadata(&self) -> Metadata {
        self.inner
            .metadata
            .iter()
            .filter(|(key, _)| !matches!(key.as_str(), "tool_call_id" | "tool_name"))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }
}

#[pymethods]
impl PyToolContext {
    #[getter]
    fn run_id(&self) -> String {
        self.inner.run_id.as_str().to_string()
    }

    #[getter]
    fn conversation_id(&self) -> String {
        self.inner.conversation_id.as_str().to_string()
    }

    #[getter]
    fn agent_id(&self) -> Option<String> {
        self.context_snapshot()
            .map(|context| context.agent_id.as_str().to_string())
    }

    #[getter]
    fn session_id(&self) -> Option<String> {
        self.context_snapshot()
            .and_then(|context| context.session_id.map(|id| id.as_str().to_string()))
    }

    #[getter]
    const fn run_step(&self) -> usize {
        self.inner.run_step
    }

    #[getter]
    const fn retry(&self) -> usize {
        self.inner.retry
    }

    #[getter]
    const fn max_retries(&self) -> usize {
        self.inner.max_retries
    }

    #[getter]
    fn metadata(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        json_to_py(py, &Value::Object(self.public_metadata()))
    }

    #[getter]
    fn run_attachments(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        json_to_py(
            py,
            &serde_json::Value::Object(self.inner.run_attachments.clone()),
        )
    }

    #[getter]
    fn context_handle(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        match self.context_handle_snapshot() {
            Some(handle) => Ok(Py::new(py, PyAgentContextHandle::new(handle))?.into_any()),
            None => Ok(py.None()),
        }
    }

    #[getter]
    fn workspace_root(&self) -> Option<String> {
        self.environment_provider()
            .and_then(|provider| provider.shell_review_context().default_cwd)
    }

    #[getter]
    fn environment(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        match self.environment_provider() {
            Some(provider) => {
                Ok(Py::new(py, PyEnvironmentProvider::new(provider.clone()))?.into_any())
            }
            None => Ok(py.None()),
        }
    }

    #[getter]
    fn raw_context(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        match self.context_snapshot() {
            Some(context) => serialize_to_py(py, &context.export_full_state()),
            None => Ok(py.None()),
        }
    }

    fn export_resources(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let Some(provider) = self.environment_provider() else {
            return Ok(py.None());
        };
        spawn_py_future(
            py,
            async move {
                provider
                    .export_state()
                    .await
                    .map(|state| state.resources)
                    .map_err(|error| PyFutureError::Runtime(error.to_string()))
            },
            |py, resources| {
                let resources = serialize_to_py(py, &resources)?;
                let registry = py
                    .import("starweaver.resources")?
                    .getattr("ResourceRegistry")?
                    .call1((resources,))?;
                Ok(registry.unbind())
            },
        )
    }

    #[getter]
    fn approval(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        match &self.inner.approval {
            Some(approval) => serialize_to_py(py, approval),
            None => Ok(py.None()),
        }
    }

    #[getter]
    fn deferred_result(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        match &self.inner.deferred_result {
            Some(result) => json_to_py(py, result),
            None => Ok(py.None()),
        }
    }

    fn is_cancelled(&self) -> bool {
        self.inner.cancellation_token.is_cancelled()
    }

    fn cancelled(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let cancellation_token = self.inner.cancellation_token();
        spawn_py_future(
            py,
            async move {
                cancellation_token.cancelled().await;
                Ok::<_, PyFutureError>(true)
            },
            |py, value| Ok(PyBool::new(py, value).to_owned().into_any().unbind()),
        )
    }
}

/// Python projection of Starweaver's toolset preparation context.
#[pyclass(name = "ToolsetContext", skip_from_py_object)]
#[derive(Clone)]
pub struct PyToolsetContext {
    inner: AgentContext,
    environment: Option<DynEnvironmentProvider>,
    environment_state: Option<EnvironmentState>,
    workspace_root: Option<String>,
}

impl PyToolsetContext {
    pub(crate) async fn from_agent_context(inner: AgentContext) -> Self {
        let environment = inner
            .dependencies
            .get::<EnvironmentHandle>()
            .map(|handle| handle.provider());
        let workspace_root = environment
            .as_ref()
            .and_then(|provider| provider.shell_review_context().default_cwd);
        let environment_state = match &environment {
            Some(provider) => provider.export_state().await.ok(),
            None => None,
        };
        Self {
            inner,
            environment,
            environment_state,
            workspace_root,
        }
    }
}

#[pymethods]
impl PyToolsetContext {
    #[getter]
    fn agent_id(&self) -> String {
        self.inner.agent_id.as_str().to_string()
    }

    #[getter]
    fn run_id(&self) -> Option<String> {
        self.inner
            .run_id
            .as_ref()
            .map(|run_id| run_id.as_str().to_string())
    }

    #[getter]
    fn session_id(&self) -> Option<String> {
        self.inner
            .session_id
            .as_ref()
            .map(|session_id| session_id.as_str().to_string())
    }

    #[getter]
    fn conversation_id(&self) -> String {
        self.inner.conversation_id.as_str().to_string()
    }

    #[getter]
    const fn run_step(&self) -> usize {
        self.inner.current_run_step
    }

    #[getter]
    fn metadata(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        json_to_py(py, &serde_json::Value::Object(self.inner.metadata.clone()))
    }

    #[getter]
    fn run_attachments(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.metadata(py)
    }

    #[getter]
    fn workspace_root(&self) -> Option<String> {
        self.workspace_root.clone()
    }

    #[getter]
    fn environment(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        match &self.environment {
            Some(provider) => {
                Ok(Py::new(py, PyEnvironmentProvider::new(provider.clone()))?.into_any())
            }
            None => Ok(py.None()),
        }
    }

    #[getter]
    fn resources(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let resources = self
            .environment_state
            .as_ref()
            .map(|state| state.resources.clone())
            .unwrap_or_default();
        let resources = serialize_to_py(py, &resources)?;
        let registry = py
            .import("starweaver.resources")?
            .getattr("ResourceRegistry")?
            .call1((resources,))?;
        Ok(registry.unbind())
    }

    #[getter]
    fn raw_context(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        serialize_to_py(py, &self.inner.export_full_state())
    }
}
