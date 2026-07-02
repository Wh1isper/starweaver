//! Python wrappers for structured output configuration.

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use pyo3::{exceptions::PyValueError, prelude::*};
use serde_json::Value;
use starweaver_model::OutputMode;
use starweaver_runtime::{
    AgentRunState, OutputFunction, OutputFunctionContext, OutputFunctionDefinition, OutputPolicy,
    OutputSchema, OutputValidationError, OutputValidationResult, OutputValidator, OutputValue,
    RunStatus,
};

use crate::conversion::{json_to_py, py_to_json, serialize_to_py};

/// Python projection of output validation and final-output function context.
#[pyclass(name = "OutputContext", skip_from_py_object)]
#[derive(Clone)]
pub struct PyOutputContext {
    state: AgentRunState,
}

impl PyOutputContext {
    const fn new(state: AgentRunState) -> Self {
        Self { state }
    }
}

#[pymethods]
impl PyOutputContext {
    #[getter]
    fn run_id(&self) -> String {
        self.state.run_id.as_str().to_string()
    }

    #[getter]
    fn conversation_id(&self) -> String {
        self.state.conversation_id.as_str().to_string()
    }

    #[getter]
    const fn run_step(&self) -> usize {
        self.state.run_step
    }

    #[getter]
    fn status(&self) -> &'static str {
        run_status_name(self.state.status)
    }

    #[getter]
    fn metadata(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        json_to_py(py, &Value::Object(self.state.metadata.clone()))
    }

    fn raw_state(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        serialize_to_py(py, &self.state)
    }
}

/// Python-visible typed final output value.
#[pyclass(name = "OutputValue", skip_from_py_object)]
#[derive(Clone, Debug)]
pub struct PyOutputValue {
    inner: OutputValue,
}

impl PyOutputValue {
    fn to_rust(&self) -> OutputValue {
        self.inner.clone()
    }
}

#[pymethods]
impl PyOutputValue {
    #[staticmethod]
    fn text(value: String) -> Self {
        Self {
            inner: OutputValue::Text(value),
        }
    }

    #[staticmethod]
    fn json(py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<Self> {
        Ok(Self {
            inner: OutputValue::Json(py_to_json(py, value)?),
        })
    }

    fn to_python(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        output_value_to_py(py, &self.inner)
    }
}

struct PythonOutputValidator {
    callback: Py<PyAny>,
    event_loop: Py<PyAny>,
}

unsafe impl Send for PythonOutputValidator {}
unsafe impl Sync for PythonOutputValidator {}

#[async_trait]
impl OutputValidator for PythonOutputValidator {
    async fn validate(
        &self,
        state: &mut AgentRunState,
        output: &OutputValue,
    ) -> OutputValidationResult<()> {
        let future = Python::attach(|py| -> PyResult<Py<PyAny>> {
            let context = Py::new(py, PyOutputContext::new(state.clone()))?;
            let output = output_value_to_py(py, output)?;
            let coroutine = self.callback.call1(py, (context, output))?;
            let asyncio = py.import("asyncio")?;
            let future = asyncio.call_method1(
                "run_coroutine_threadsafe",
                (coroutine, self.event_loop.clone_ref(py)),
            )?;
            Ok(future.unbind())
        })
        .map_err(py_error_to_output_validation_error)?;

        let value = await_python_future(future)
            .await
            .map_err(py_error_to_output_validation_error)?;
        Python::attach(|py| -> OutputValidationResult<()> {
            let value = value.bind(py);
            if value.is_none() {
                return Ok(());
            }
            if let Ok(accepted) = value.extract::<bool>()
                && accepted
            {
                return Ok(());
            }
            Err(OutputValidationError::failed(
                "output validator must return None or True",
            ))
        })
    }
}

/// Python-backed output validator handle.
#[pyclass(name = "OutputValidator", skip_from_py_object)]
#[derive(Clone)]
pub struct PyOutputValidator {
    inner: Arc<PythonOutputValidator>,
}

impl PyOutputValidator {
    pub(crate) fn dyn_validator(&self) -> Arc<dyn OutputValidator> {
        self.inner.clone()
    }
}

#[pymethods]
impl PyOutputValidator {
    #[new]
    #[pyo3(signature = (callback, event_loop))]
    fn new(callback: Py<PyAny>, event_loop: Py<PyAny>) -> Self {
        Self {
            inner: Arc::new(PythonOutputValidator {
                callback,
                event_loop,
            }),
        }
    }
}

struct PythonOutputFunction {
    definition: OutputFunctionDefinition,
    callback: Py<PyAny>,
    event_loop: Py<PyAny>,
}

unsafe impl Send for PythonOutputFunction {}
unsafe impl Sync for PythonOutputFunction {}

#[async_trait]
impl OutputFunction for PythonOutputFunction {
    fn definition(&self) -> OutputFunctionDefinition {
        self.definition.clone()
    }

    async fn call(
        &self,
        context: OutputFunctionContext,
        arguments: Value,
    ) -> OutputValidationResult<OutputValue> {
        let future = Python::attach(|py| -> PyResult<Py<PyAny>> {
            let context = Py::new(py, PyOutputContext::new(context.state.clone()))?;
            let arguments = json_to_py(py, &arguments)?;
            let coroutine = self.callback.call1(py, (context, arguments))?;
            let asyncio = py.import("asyncio")?;
            let future = asyncio.call_method1(
                "run_coroutine_threadsafe",
                (coroutine, self.event_loop.clone_ref(py)),
            )?;
            Ok(future.unbind())
        })
        .map_err(py_error_to_output_validation_error)?;

        let value = await_python_future(future)
            .await
            .map_err(py_error_to_output_validation_error)?;
        Python::attach(|py| py_value_to_output_value(py, value.bind(py)))
    }
}

/// Python-backed output function handle.
#[pyclass(name = "OutputFunction", skip_from_py_object)]
#[derive(Clone)]
pub struct PyOutputFunction {
    inner: Arc<PythonOutputFunction>,
}

impl PyOutputFunction {
    pub(crate) fn dyn_function(&self) -> Arc<dyn OutputFunction> {
        self.inner.clone()
    }
}

#[pymethods]
impl PyOutputFunction {
    #[new]
    #[pyo3(signature = (name, parameters_schema, callback, event_loop, description=None))]
    fn new(
        py: Python<'_>,
        name: String,
        parameters_schema: &Bound<'_, PyAny>,
        callback: Py<PyAny>,
        event_loop: Py<PyAny>,
        description: Option<String>,
    ) -> PyResult<Self> {
        if name.trim().is_empty() {
            return Err(PyValueError::new_err(
                "output function name must not be empty",
            ));
        }
        let parameters = py_to_json(py, parameters_schema)?;
        if !parameters.is_object() {
            return Err(PyValueError::new_err(
                "output function parameters_schema must be a JSON object",
            ));
        }
        let mut definition = OutputFunctionDefinition::new(name, parameters);
        if let Some(description) = description {
            definition = definition.with_description(description);
        }
        Ok(Self {
            inner: Arc::new(PythonOutputFunction {
                definition,
                callback,
                event_loop,
            }),
        })
    }

    fn definition_json(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        serialize_to_py(py, &self.inner.definition)
    }
}

/// Python wrapper around a Starweaver structured output schema.
#[pyclass(name = "OutputSchema", skip_from_py_object)]
#[derive(Clone)]
pub struct PyOutputSchema {
    inner: OutputSchema,
}

impl PyOutputSchema {
    pub(crate) fn schema(&self) -> OutputSchema {
        self.inner.clone()
    }
}

#[pymethods]
impl PyOutputSchema {
    #[new]
    #[pyo3(signature = (name, schema, description=None, strict=true))]
    fn new(
        py: Python<'_>,
        name: String,
        schema: &Bound<'_, PyAny>,
        description: Option<String>,
        strict: bool,
    ) -> PyResult<Self> {
        let mut inner = OutputSchema::new(name, py_to_json(py, schema)?).with_strict(strict);
        if let Some(description) = description {
            inner = inner.with_description(description);
        }
        Ok(Self { inner })
    }

    fn request_schema(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        json_to_py(py, &self.inner.request_schema())
    }

    fn to_dict(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        serialize_to_py(py, &self.inner)
    }
}

/// Python wrapper around a Starweaver output policy.
#[pyclass(name = "OutputPolicy", skip_from_py_object)]
#[derive(Clone)]
pub struct PyOutputPolicy {
    inner: OutputPolicy,
}

impl PyOutputPolicy {
    pub(crate) fn policy(&self) -> OutputPolicy {
        self.inner.clone()
    }
}

#[pymethods]
impl PyOutputPolicy {
    #[new]
    fn new() -> Self {
        Self {
            inner: OutputPolicy::new(),
        }
    }

    #[staticmethod]
    fn text() -> Self {
        Self {
            inner: OutputPolicy::text(),
        }
    }

    #[staticmethod]
    fn structured(py: Python<'_>, schema: &Bound<'_, PyAny>) -> PyResult<Self> {
        Ok(Self {
            inner: OutputPolicy::structured(
                extract_output_schema(py, Some(schema))?
                    .ok_or_else(|| PyValueError::new_err("schema is required"))?,
            ),
        })
    }

    #[staticmethod]
    fn auto(py: Python<'_>, schema: &Bound<'_, PyAny>) -> PyResult<Self> {
        Ok(Self {
            inner: OutputPolicy::auto(
                extract_output_schema(py, Some(schema))?
                    .ok_or_else(|| PyValueError::new_err("schema is required"))?,
            ),
        })
    }

    #[staticmethod]
    fn native_json_schema(py: Python<'_>, schema: &Bound<'_, PyAny>) -> PyResult<Self> {
        Ok(Self {
            inner: OutputPolicy::native_json_schema(
                extract_output_schema(py, Some(schema))?
                    .ok_or_else(|| PyValueError::new_err("schema is required"))?,
            ),
        })
    }

    #[staticmethod]
    fn native_json_object(py: Python<'_>, schema: &Bound<'_, PyAny>) -> PyResult<Self> {
        Ok(Self {
            inner: OutputPolicy::native_json_object(
                extract_output_schema(py, Some(schema))?
                    .ok_or_else(|| PyValueError::new_err("schema is required"))?,
            ),
        })
    }

    #[staticmethod]
    fn tool(py: Python<'_>, schema: &Bound<'_, PyAny>) -> PyResult<Self> {
        Ok(Self {
            inner: OutputPolicy::tool(
                extract_output_schema(py, Some(schema))?
                    .ok_or_else(|| PyValueError::new_err("schema is required"))?,
            ),
        })
    }

    #[staticmethod]
    fn tool_or_text(py: Python<'_>, schema: &Bound<'_, PyAny>) -> PyResult<Self> {
        Ok(Self {
            inner: OutputPolicy::tool_or_text(
                extract_output_schema(py, Some(schema))?
                    .ok_or_else(|| PyValueError::new_err("schema is required"))?,
            ),
        })
    }

    #[staticmethod]
    fn prompted(py: Python<'_>, schema: &Bound<'_, PyAny>) -> PyResult<Self> {
        Ok(Self {
            inner: OutputPolicy::prompted(
                extract_output_schema(py, Some(schema))?
                    .ok_or_else(|| PyValueError::new_err("schema is required"))?,
            ),
        })
    }

    #[staticmethod]
    fn image() -> Self {
        Self {
            inner: OutputPolicy::image(),
        }
    }

    fn with_retries(&self, retries: usize) -> Self {
        Self {
            inner: self.inner.clone().with_retries(retries),
        }
    }

    fn with_mode(&self, mode: String) -> PyResult<Self> {
        Ok(Self {
            inner: self.inner.clone().with_mode(parse_output_mode(&mode)?),
        })
    }

    fn allow_text_output(&self, allow: bool) -> Self {
        Self {
            inner: self.inner.clone().allow_text_output(allow),
        }
    }

    fn allow_image_output(&self, allow: bool) -> Self {
        Self {
            inner: self.inner.clone().allow_image_output(allow),
        }
    }

    fn with_validator(&self, py: Python<'_>, validator: Py<PyOutputValidator>) -> Self {
        Self {
            inner: self
                .inner
                .clone()
                .with_validator(validator.borrow(py).dyn_validator()),
        }
    }

    fn with_function(&self, py: Python<'_>, function: Py<PyOutputFunction>) -> Self {
        Self {
            inner: self
                .inner
                .clone()
                .with_function(function.borrow(py).dyn_function()),
        }
    }
}

pub(crate) fn extract_output_schema(
    py: Python<'_>,
    value: Option<&Bound<'_, PyAny>>,
) -> PyResult<Option<OutputSchema>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_none() {
        return Ok(None);
    }
    if let Ok(schema) = value.extract::<PyRef<'_, PyOutputSchema>>() {
        return Ok(Some(schema.schema()));
    }
    match py_to_json(py, value)? {
        Value::Object(mut object) => {
            if object.contains_key("name") && object.contains_key("schema") {
                serde_json::from_value(Value::Object(object))
                    .map(Some)
                    .map_err(|error| {
                        PyValueError::new_err(format!("invalid output schema: {error}"))
                    })
            } else {
                Ok(Some(OutputSchema::new(
                    "output",
                    Value::Object(std::mem::take(&mut object)),
                )))
            }
        }
        other => Err(PyValueError::new_err(format!(
            "output schema must be a mapping, got {other:?}"
        ))),
    }
}

pub(crate) fn extract_output_policy(
    py: Python<'_>,
    value: Option<&Bound<'_, PyAny>>,
) -> PyResult<Option<OutputPolicy>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_none() {
        return Ok(None);
    }
    if let Ok(policy) = value.extract::<PyRef<'_, PyOutputPolicy>>() {
        return Ok(Some(policy.policy()));
    }
    let Value::Object(mut object) = py_to_json(py, value)? else {
        return Err(PyValueError::new_err("output policy must be a mapping"));
    };
    let schema = object
        .remove("schema")
        .map(parse_schema_value)
        .transpose()?;
    let mode = object
        .remove("mode")
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .map(|value| parse_output_mode(&value))
        .transpose()?;
    let retries = object
        .remove("retries")
        .and_then(|value| value.as_u64())
        .map(|value| value as usize);
    let allow_text_output = object
        .remove("allow_text_output")
        .and_then(|value| value.as_bool());
    let allow_image_output = object
        .remove("allow_image_output")
        .and_then(|value| value.as_bool());
    let mut policy = schema.map_or_else(OutputPolicy::new, OutputPolicy::structured);
    if let Some(mode) = mode {
        policy = policy.with_mode(mode);
    }
    if let Some(retries) = retries {
        policy = policy.with_retries(retries);
    }
    if let Some(allow) = allow_text_output {
        policy = policy.allow_text_output(allow);
    }
    if let Some(allow) = allow_image_output {
        policy = policy.allow_image_output(allow);
    }
    Ok(Some(policy))
}

fn parse_schema_value(value: Value) -> PyResult<OutputSchema> {
    match value {
        Value::Object(object) if object.contains_key("name") && object.contains_key("schema") => {
            serde_json::from_value(Value::Object(object)).map_err(|error| {
                PyValueError::new_err(format!("invalid output policy schema: {error}"))
            })
        }
        value => Ok(OutputSchema::new("output", value)),
    }
}

fn parse_output_mode(mode: &str) -> PyResult<OutputMode> {
    match mode {
        "auto" => Ok(OutputMode::Auto),
        "text" => Ok(OutputMode::Text),
        "native_json_schema" => Ok(OutputMode::NativeJsonSchema),
        "native_json_object" => Ok(OutputMode::NativeJsonObject),
        "tool" => Ok(OutputMode::Tool),
        "tool_or_text" => Ok(OutputMode::ToolOrText),
        "prompted" => Ok(OutputMode::Prompted),
        "image" => Ok(OutputMode::Image),
        other => Err(PyValueError::new_err(format!(
            "unsupported output mode: {other}"
        ))),
    }
}

async fn await_python_future(future: Py<PyAny>) -> PyResult<Py<PyAny>> {
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
    result
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

fn output_value_to_py(py: Python<'_>, output: &OutputValue) -> PyResult<Py<PyAny>> {
    match output {
        OutputValue::Text(text) => json_to_py(py, &Value::String(text.clone())),
        OutputValue::Json(value) => json_to_py(py, value),
        OutputValue::Media(media) => serialize_to_py(py, media),
    }
}

fn py_value_to_output_value(
    py: Python<'_>,
    value: &Bound<'_, PyAny>,
) -> OutputValidationResult<OutputValue> {
    if let Ok(output) = value.extract::<PyRef<'_, PyOutputValue>>() {
        return Ok(output.to_rust());
    }
    if let Ok(text) = value.extract::<String>() {
        return Ok(OutputValue::Text(text));
    }
    py_to_json(py, value)
        .map(OutputValue::Json)
        .map_err(|error| OutputValidationError::failed(error.to_string()))
}

fn py_error_to_output_validation_error(error: PyErr) -> OutputValidationError {
    Python::attach(|py| {
        let type_name = error
            .get_type(py)
            .name()
            .map_or_else(|_| "Exception".to_string(), |name| name.to_string());
        let message = error.to_string();

        if let Some(kind) = starweaver_output_error_kind(py, &error) {
            return match kind {
                "Retry" => OutputValidationError::retry(message),
                "Failed" => OutputValidationError::failed(message),
                _ => OutputValidationError::failed(message),
            };
        }

        match type_name.as_str() {
            "ValidationError" => OutputValidationError::retry(message),
            "CancelledError" => OutputValidationError::failed(message),
            _ => OutputValidationError::failed(format!("{type_name}: {message}")),
        }
    })
}

fn starweaver_output_error_kind<'a>(py: Python<'a>, error: &PyErr) -> Option<&'static str> {
    let module = py.import("starweaver.errors").ok()?;
    for (class_name, kind) in [
        ("OutputRetry", "Retry"),
        ("ModelRetry", "Retry"),
        ("OutputValidationFailed", "Failed"),
    ] {
        let class = module.getattr(class_name).ok()?;
        if error.value(py).is_instance(&class).unwrap_or(false) {
            return Some(kind);
        }
    }
    None
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
