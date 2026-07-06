//! Python context projections.

use pyo3::{prelude::*, types::PyBool};
use starweaver_agent::EnvironmentHandle;
use starweaver_context::AgentContext;
use starweaver_environment::{DynEnvironmentProvider, EnvironmentState};
use starweaver_tools::ToolContext;

use crate::{
    conversion::{json_to_py, serialize_to_py},
    environment::PyEnvironmentProvider,
    runtime::{PyFutureError, spawn_py_future},
};

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
        json_to_py(py, &serde_json::Value::Object(self.inner.metadata.clone()))
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
