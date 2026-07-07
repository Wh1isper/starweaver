//! Python-backed Starweaver tools.

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use pyo3::{
    exceptions::{PyRuntimeError, PyValueError},
    prelude::*,
};
use serde_json::Value;
use starweaver_core::Metadata;
use starweaver_tools::{DynTool, Tool, ToolContext, ToolError, ToolResult};

use crate::{
    context::PyToolContext,
    conversion::{json_to_py, optional_py_to_json, optional_py_to_metadata, py_to_json},
};

/// Python-visible Starweaver tool result.
#[pyclass(name = "ToolResult", skip_from_py_object)]
#[derive(Clone, Debug)]
pub struct PyToolResult {
    content: Value,
    metadata: Metadata,
    app_value: Option<Value>,
    model_content: Option<Value>,
    user_content: Option<Value>,
    private_metadata: Metadata,
}

impl PyToolResult {
    fn to_rust(&self) -> ToolResult {
        let mut result = ToolResult::new(self.content.clone()).with_metadata(self.metadata.clone());
        if let Some(app_value) = &self.app_value {
            result = result.with_app_value(app_value.clone());
        }
        if let Some(model_content) = &self.model_content {
            result = result.with_model_content(model_content.clone());
        }
        if let Some(user_content) = &self.user_content {
            result = result.with_user_content(user_content.clone());
        }
        if !self.private_metadata.is_empty() {
            result = result.with_private_metadata(self.private_metadata.clone());
        }
        result
    }
}

#[pymethods]
impl PyToolResult {
    #[new]
    #[pyo3(signature = (content, metadata=None, app_value=None, model_content=None, user_content=None, private_metadata=None))]
    fn new(
        py: Python<'_>,
        content: &Bound<'_, PyAny>,
        metadata: Option<&Bound<'_, PyAny>>,
        app_value: Option<&Bound<'_, PyAny>>,
        model_content: Option<&Bound<'_, PyAny>>,
        user_content: Option<&Bound<'_, PyAny>>,
        private_metadata: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Self> {
        Ok(Self {
            content: py_to_json(py, content)?,
            metadata: optional_py_to_metadata(py, metadata)?,
            app_value: optional_py_to_json(py, app_value)?,
            model_content: optional_py_to_json(py, model_content)?,
            user_content: optional_py_to_json(py, user_content)?,
            private_metadata: optional_py_to_metadata(py, private_metadata)?,
        })
    }

    #[getter]
    fn content(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        json_to_py(py, &self.content)
    }

    #[getter]
    fn metadata(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        json_to_py(py, &Value::Object(self.metadata.clone()))
    }

    #[getter]
    fn app_value(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        match &self.app_value {
            Some(value) => json_to_py(py, value),
            None => Ok(py.None()),
        }
    }

    #[getter]
    fn model_content(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        match &self.model_content {
            Some(value) => json_to_py(py, value),
            None => Ok(py.None()),
        }
    }

    #[getter]
    fn user_content(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        match &self.user_content {
            Some(value) => json_to_py(py, value),
            None => Ok(py.None()),
        }
    }

    #[getter]
    fn private_metadata(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        json_to_py(py, &Value::Object(self.private_metadata.clone()))
    }
}

struct PythonTool {
    name: String,
    description: Option<String>,
    parameters_schema: Value,
    return_schema: Option<Value>,
    metadata: Metadata,
    strict: Option<bool>,
    sequential: Option<bool>,
    timeout_ms: Option<u64>,
    max_retries: Option<usize>,
    callback: Py<PyAny>,
    event_loop: Py<PyAny>,
}

unsafe impl Send for PythonTool {}
unsafe impl Sync for PythonTool {}

impl PythonTool {
    async fn dispatch(
        &self,
        context: ToolContext,
        arguments: Value,
    ) -> Result<ToolResult, ToolError> {
        let call_result = Python::attach(|py| -> PyResult<Py<PyAny>> {
            let ctx = Py::new(py, PyToolContext::new(context.clone()))?;
            let args = json_to_py(py, &arguments)?;
            let coroutine = self.callback.call1(py, (ctx, args))?;
            let asyncio = py.import("asyncio")?;
            let future = asyncio.call_method1(
                "run_coroutine_threadsafe",
                (coroutine, self.event_loop.clone_ref(py)),
            )?;
            Ok(future.unbind())
        });
        let future = match call_result {
            Ok(future) => future,
            Err(error) => {
                return Err(py_error_to_tool_error(&self.name, self.timeout_ms, error));
            }
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
        };
        cancel_guard.complete();

        match result {
            Ok(value) => {
                Python::attach(|py| py_value_to_tool_result(py, value.bind(py), &self.name))
            }
            Err(error) => Err(py_error_to_tool_error(&self.name, self.timeout_ms, error)),
        }
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

#[async_trait]
impl Tool for PythonTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    fn parameters_schema(&self) -> Value {
        self.parameters_schema.clone()
    }

    fn metadata(&self) -> Metadata {
        self.metadata.clone()
    }

    fn max_retries(&self) -> Option<usize> {
        self.max_retries
    }

    fn timeout_ms(&self) -> Option<u64> {
        self.timeout_ms
    }

    fn return_schema(&self) -> Option<Value> {
        self.return_schema.clone()
    }

    fn strict_schema(&self) -> Option<bool> {
        self.strict
    }

    fn sequential(&self) -> Option<bool> {
        self.sequential
    }

    async fn call(&self, context: ToolContext, arguments: Value) -> Result<ToolResult, ToolError> {
        self.dispatch(context, arguments).await
    }
}

/// Native handle for a Python-backed Starweaver tool.
#[pyclass(name = "PythonTool", skip_from_py_object)]
#[derive(Clone)]
pub struct PyPythonTool {
    inner: Arc<PythonTool>,
}

impl PyPythonTool {
    pub(crate) fn dyn_tool(&self) -> DynTool {
        self.inner.clone()
    }
}

pub(crate) fn py_tool_list_to_dyn_tools(
    py: Python<'_>,
    tools: Option<Vec<Py<PyPythonTool>>>,
) -> PyResult<Vec<DynTool>> {
    let mut result = Vec::new();
    for tool in tools.unwrap_or_default() {
        result.push(tool.borrow(py).dyn_tool());
    }
    Ok(result)
}

#[pymethods]
impl PyPythonTool {
    #[new]
    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (name, description, parameters_schema, callback, event_loop, return_schema=None, metadata=None, strict=None, sequential=None, timeout_ms=None, max_retries=None))]
    fn new(
        py: Python<'_>,
        name: String,
        description: Option<String>,
        parameters_schema: &Bound<'_, PyAny>,
        callback: Py<PyAny>,
        event_loop: Py<PyAny>,
        return_schema: Option<&Bound<'_, PyAny>>,
        metadata: Option<&Bound<'_, PyAny>>,
        strict: Option<bool>,
        sequential: Option<bool>,
        timeout_ms: Option<u64>,
        max_retries: Option<usize>,
    ) -> PyResult<Self> {
        if name.trim().is_empty() {
            return Err(PyValueError::new_err("tool name must not be empty"));
        }
        let parameters_schema = py_to_json(py, parameters_schema)?;
        if !parameters_schema.is_object() {
            return Err(PyValueError::new_err(
                "parameters_schema must be a JSON object",
            ));
        }
        Ok(Self {
            inner: Arc::new(PythonTool {
                name,
                description,
                parameters_schema,
                return_schema: optional_py_to_json(py, return_schema)?,
                metadata: optional_py_to_metadata(py, metadata)?,
                strict,
                sequential,
                timeout_ms,
                max_retries,
                callback,
                event_loop,
            }),
        })
    }

    #[getter]
    fn name(&self) -> String {
        self.inner.name.clone()
    }

    fn definition_json(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let definition = self.inner.definition();
        let value = serde_json::to_value(&definition).map_err(|error| {
            PyRuntimeError::new_err(format!("failed to serialize tool definition: {error}"))
        })?;
        json_to_py(py, &value)
    }
}

fn py_value_to_tool_result(
    py: Python<'_>,
    value: &Bound<'_, PyAny>,
    tool: &str,
) -> Result<ToolResult, ToolError> {
    if let Ok(result) = value.extract::<PyRef<'_, PyToolResult>>() {
        return Ok(result.to_rust());
    }
    py_to_json(py, value)
        .map(ToolResult::new)
        .map_err(|error| ToolError::Execution {
            tool: tool.to_string(),
            message: error.to_string(),
        })
}

fn py_error_to_tool_error(tool: &str, timeout_ms: Option<u64>, error: PyErr) -> ToolError {
    Python::attach(|py| {
        let type_name = error
            .get_type(py)
            .name()
            .map_or_else(|_| "Exception".to_string(), |name| name.to_string());
        let message = error.to_string();
        let private_metadata = python_error_metadata(py, &error, &type_name);

        if let Some(kind) = starweaver_tool_error_kind(py, &error) {
            return match kind {
                "InvalidArguments" => ToolError::InvalidArguments {
                    tool: tool.to_string(),
                    message,
                }
                .with_private_metadata(private_metadata),
                "ModelRetry" => ToolError::ModelRetry {
                    tool: tool.to_string(),
                    message,
                }
                .with_private_metadata(private_metadata),
                "Feedback" => ToolError::Feedback {
                    tool: tool.to_string(),
                    message,
                }
                .with_private_metadata(private_metadata),
                "UserError" => ToolError::UserError {
                    tool: tool.to_string(),
                    message,
                }
                .with_private_metadata(private_metadata),
                "ApprovalRequired" => ToolError::ApprovalRequired {
                    tool: tool.to_string(),
                    metadata: exception_metadata(py, &error),
                }
                .with_private_metadata(private_metadata),
                "CallDeferred" => ToolError::CallDeferred {
                    tool: tool.to_string(),
                    metadata: exception_metadata(py, &error),
                }
                .with_private_metadata(private_metadata),
                "Cancelled" => ToolError::Cancelled {
                    tool: tool.to_string(),
                    reason: message,
                }
                .with_private_metadata(private_metadata),
                "Timeout" => ToolError::Timeout {
                    tool: tool.to_string(),
                    timeout_ms: timeout_ms.unwrap_or_default(),
                }
                .with_private_metadata(private_metadata),
                _ => ToolError::Execution {
                    tool: tool.to_string(),
                    message,
                }
                .with_private_metadata(private_metadata),
            };
        }

        match type_name.as_str() {
            "ValidationError" => ToolError::InvalidArguments {
                tool: tool.to_string(),
                message,
            }
            .with_private_metadata(private_metadata),
            "CancelledError" => ToolError::Cancelled {
                tool: tool.to_string(),
                reason: message,
            }
            .with_private_metadata(private_metadata),
            "TimeoutError" => ToolError::Timeout {
                tool: tool.to_string(),
                timeout_ms: timeout_ms.unwrap_or_default(),
            }
            .with_private_metadata(private_metadata),
            _ => ToolError::Execution {
                tool: tool.to_string(),
                message,
            }
            .with_private_metadata(private_metadata),
        }
    })
}

fn starweaver_tool_error_kind<'a>(py: Python<'a>, error: &PyErr) -> Option<&'static str> {
    let module = py.import("starweaver.errors").ok()?;
    for (class_name, kind) in [
        ("InvalidArguments", "InvalidArguments"),
        ("ModelRetry", "ModelRetry"),
        ("Feedback", "Feedback"),
        ("UserError", "UserError"),
        ("ApprovalRequired", "ApprovalRequired"),
        ("CallDeferred", "CallDeferred"),
        ("Cancelled", "Cancelled"),
        ("Timeout", "Timeout"),
    ] {
        let class = module.getattr(class_name).ok()?;
        if error.value(py).is_instance(&class).unwrap_or(false) {
            return Some(kind);
        }
    }
    None
}

fn exception_metadata(py: Python<'_>, error: &PyErr) -> Value {
    let value = error.value(py);
    let mut metadata = match value.getattr("metadata") {
        Ok(metadata) => {
            py_to_json(py, &metadata).unwrap_or_else(|_| Value::Object(Metadata::new()))
        }
        Err(_) => Value::Object(Metadata::new()),
    };
    if let Value::Object(object) = &mut metadata
        && let Ok(reason) = value
            .getattr("reason")
            .and_then(|reason| reason.extract::<String>())
    {
        object.insert("reason".to_string(), Value::String(reason));
    }
    metadata
}

fn python_error_metadata(py: Python<'_>, error: &PyErr, type_name: &str) -> Metadata {
    let mut metadata = Metadata::new();
    metadata.insert(
        "python_exception_type".to_string(),
        Value::String(type_name.to_string()),
    );
    metadata.insert(
        "python_exception".to_string(),
        Value::String(error.to_string()),
    );
    if let Ok(traceback) = format_python_exception(py, error) {
        metadata.insert("python_traceback".to_string(), Value::String(traceback));
    }
    metadata
}

fn format_python_exception(py: Python<'_>, error: &PyErr) -> PyResult<String> {
    let traceback = py.import("traceback")?;
    let formatted = traceback.call_method1(
        "format_exception",
        (error.get_type(py), error.value(py), error.traceback(py)),
    )?;
    let lines: Vec<String> = formatted.extract()?;
    Ok(lines.join(""))
}
