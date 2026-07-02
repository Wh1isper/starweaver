//! Python tool context projection.

use pyo3::prelude::*;
use starweaver_tools::ToolContext;

use crate::conversion::{json_to_py, serialize_to_py};

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
}
