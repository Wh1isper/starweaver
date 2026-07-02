//! Python testing helpers backed by deterministic Rust models.

use std::{sync::Arc, time::Duration};

use pyo3::{
    exceptions::{PyTypeError, PyValueError},
    prelude::*,
};
use serde_json::Value;
use starweaver_model::{
    FunctionModel, FunctionModelInfo, ModelAdapter, ModelError, ModelMessage, ModelResponse,
    ModelResponsePart, TestModel, ToolArguments, ToolCallPart,
};

use crate::{
    conversion::{json_to_py, py_to_json, serialize_to_py},
    model::PyProviderModel,
    runtime::{PyFutureError, spawn_py_future},
};

/// Sleep on the native Tokio runtime and echo a JSON value.
#[pyfunction]
#[pyo3(signature = (value, delay_ms=0))]
pub(crate) fn sleep_echo(
    py: Python<'_>,
    value: &Bound<'_, PyAny>,
    delay_ms: u64,
) -> PyResult<Py<PyAny>> {
    let value = py_to_json(py, value)?;
    spawn_py_future(
        py,
        async move {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            Ok::<_, PyFutureError>(value)
        },
        |py, value| json_to_py(py, &value),
    )
}

/// Deterministic test model for Python SDK tests and examples.
#[pyclass(name = "TestModel", skip_from_py_object)]
#[derive(Clone)]
pub struct PyTestModel {
    inner: TestModel,
}

impl PyTestModel {
    pub(crate) fn model(&self) -> Arc<dyn ModelAdapter> {
        Arc::new(self.inner.clone())
    }
}

/// Extract a supported native model adapter from a Python model handle.
pub(crate) fn py_model_from_any(model: &Bound<'_, PyAny>) -> PyResult<Arc<dyn ModelAdapter>> {
    if let Ok(model) = model.extract::<PyRef<'_, PyTestModel>>() {
        return Ok(model.model());
    }
    if let Ok(model) = model.extract::<PyRef<'_, PyFunctionModel>>() {
        return Ok(model.model());
    }
    if let Ok(model) = model.extract::<PyRef<'_, PyProviderModel>>() {
        return Ok(model.model());
    }
    Err(PyTypeError::new_err(
        "model must be a starweaver.testing.TestModel, FunctionModel, or ProviderModel",
    ))
}

#[pymethods]
impl PyTestModel {
    #[new]
    #[pyo3(signature = (text=None, responses=None))]
    fn new(
        py: Python<'_>,
        text: Option<String>,
        responses: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Self> {
        if let Some(responses) = responses {
            let response_value = py_to_json(py, responses)?;
            let responses = parse_response_list(response_value)?;
            return Ok(Self {
                inner: TestModel::with_responses(responses),
            });
        }
        Ok(Self {
            inner: TestModel::with_text(text.unwrap_or_else(|| "ok".to_string())),
        })
    }

    #[staticmethod]
    fn text(text: String) -> Self {
        Self {
            inner: TestModel::with_text(text),
        }
    }

    #[staticmethod]
    fn responses(py: Python<'_>, responses: &Bound<'_, PyAny>) -> PyResult<Self> {
        let responses = parse_response_list(py_to_json(py, responses)?)?;
        Ok(Self {
            inner: TestModel::with_responses(responses),
        })
    }

    fn captured_messages(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        serialize_to_py(py, &self.inner.captured_messages())
    }

    fn captured_params(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        serialize_to_py(py, &self.inner.captured_params())
    }

    #[staticmethod]
    fn tool_call_response(py: Python<'_>, calls: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        let value = serde_json::json!({"tool_calls": py_to_json(py, calls)?});
        json_to_py(py, &value)
    }
}

/// Deterministic model backed by a Python callback.
#[pyclass(name = "FunctionModel", skip_from_py_object)]
#[derive(Clone)]
pub struct PyFunctionModel {
    inner: FunctionModel,
}

impl PyFunctionModel {
    pub(crate) fn model(&self) -> Arc<dyn ModelAdapter> {
        Arc::new(self.inner.clone())
    }
}

#[pymethods]
impl PyFunctionModel {
    #[new]
    #[pyo3(signature = (callback, event_loop, model_name=None))]
    fn new(
        py: Python<'_>,
        callback: Py<PyAny>,
        event_loop: Py<PyAny>,
        model_name: Option<String>,
    ) -> Self {
        let callback_for_request = callback.clone_ref(py);
        let event_loop_for_request = event_loop.clone_ref(py);
        let mut inner = FunctionModel::new(move |messages, settings, info| {
            call_python_function_model(
                &callback_for_request,
                &event_loop_for_request,
                messages,
                settings,
                info,
            )
        });
        if let Some(model_name) = model_name {
            inner = inner.with_model_name(model_name);
        }
        Self { inner }
    }

    fn captured_messages(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        serialize_to_py(py, &self.inner.captured_messages())
    }

    fn captured_params(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        serialize_to_py(py, &self.inner.captured_params())
    }
}

fn call_python_function_model(
    callback: &Py<PyAny>,
    event_loop: &Py<PyAny>,
    messages: Vec<ModelMessage>,
    settings: Option<starweaver_model::ModelSettings>,
    info: FunctionModelInfo,
) -> Result<ModelResponse, ModelError> {
    Python::attach(|py| {
        let py_messages = serialize_to_py(py, &messages).map_err(py_result_to_model_error)?;
        let py_info = serialize_to_py(
            py,
            &serde_json::json!({
                "params": info.params,
                "settings": settings,
                "context": info.context,
            }),
        )
        .map_err(py_result_to_model_error)?;
        let value = callback
            .call1(py, (py_messages, py_info))
            .map_err(py_error_to_model_error)?;
        let value =
            resolve_model_callback_value(py, value, event_loop).map_err(py_error_to_model_error)?;
        let response = py_to_json(py, value.bind(py)).map_err(py_result_to_model_error)?;
        parse_response(response).map_err(py_result_to_model_error)
    })
}

fn resolve_model_callback_value(
    py: Python<'_>,
    value: Py<PyAny>,
    event_loop: &Py<PyAny>,
) -> PyResult<Py<PyAny>> {
    let inspect = py.import("inspect")?;
    let is_awaitable: bool = inspect.call_method1("isawaitable", (&value,))?.extract()?;
    if !is_awaitable {
        return Ok(value);
    }
    let asyncio = py.import("asyncio")?;
    let future = asyncio.call_method1(
        "run_coroutine_threadsafe",
        (value, event_loop.clone_ref(py)),
    )?;
    Ok(future.call_method0("result")?.unbind())
}

fn py_error_to_model_error(error: PyErr) -> ModelError {
    ModelError::Transport(error.to_string())
}

fn py_result_to_model_error(error: PyErr) -> ModelError {
    ModelError::ResponseParsing(error.to_string())
}

fn parse_response_list(value: Value) -> PyResult<Vec<ModelResponse>> {
    match value {
        Value::Array(items) => items.into_iter().map(parse_response).collect(),
        other => Err(PyValueError::new_err(format!(
            "responses must be a list, got {other:?}"
        ))),
    }
}

fn parse_response(value: Value) -> PyResult<ModelResponse> {
    match value {
        Value::String(text) => Ok(ModelResponse::text(text)),
        Value::Object(mut object) => {
            if let Some(text) = object.remove("text") {
                return Ok(ModelResponse::text(
                    text.as_str()
                        .ok_or_else(|| PyValueError::new_err("response text must be a string"))?
                        .to_string(),
                ));
            }
            if let Some(tool_calls) = object.remove("tool_calls") {
                let calls = parse_tool_calls(tool_calls)?;
                return Ok(ModelResponse {
                    parts: calls.into_iter().map(ModelResponsePart::ToolCall).collect(),
                    ..ModelResponse::text("")
                });
            }
            Err(PyValueError::new_err(
                "response objects must contain 'text' or 'tool_calls'",
            ))
        }
        other => Err(PyValueError::new_err(format!(
            "unsupported response value: {other:?}"
        ))),
    }
}

fn parse_tool_calls(value: Value) -> PyResult<Vec<ToolCallPart>> {
    let Value::Array(items) = value else {
        return Err(PyValueError::new_err("tool_calls must be a list"));
    };
    items.into_iter().map(parse_tool_call).collect()
}

fn parse_tool_call(value: Value) -> PyResult<ToolCallPart> {
    let Value::Object(mut object) = value else {
        return Err(PyValueError::new_err("tool call must be an object"));
    };
    let id = object
        .remove("id")
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .ok_or_else(|| PyValueError::new_err("tool call requires string id"))?;
    let name = object
        .remove("name")
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .ok_or_else(|| PyValueError::new_err("tool call requires string name"))?;
    let arguments = object
        .remove("arguments")
        .unwrap_or(Value::Object(Default::default()));
    Ok(ToolCallPart {
        id,
        name,
        arguments: ToolArguments::from(arguments),
    })
}
