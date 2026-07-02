//! Tokio runtime and Python future bridging.

use std::{future::Future, sync::OnceLock};

use pyo3::{exceptions::PyRuntimeError, prelude::*};
use starweaver_runtime::AgentError;
use tokio::runtime::{Builder, Runtime};

static TOKIO_RUNTIME: OnceLock<Runtime> = OnceLock::new();

fn tokio_runtime() -> PyResult<&'static Runtime> {
    if let Some(runtime) = TOKIO_RUNTIME.get() {
        return Ok(runtime);
    }
    let runtime = Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .thread_name("starweaver-py")
        .build()
        .map_err(|error| PyRuntimeError::new_err(format!("failed to start runtime: {error}")))?;
    let _ = TOKIO_RUNTIME.set(runtime);
    TOKIO_RUNTIME
        .get()
        .ok_or_else(|| PyRuntimeError::new_err("failed to initialize runtime"))
}

pub(crate) fn enter_runtime<T>(f: impl FnOnce() -> PyResult<T>) -> PyResult<T> {
    let handle = tokio_runtime()?.handle().clone();
    let _guard = handle.enter();
    f()
}

#[derive(Clone, Debug)]
pub(crate) enum PyFutureError {
    Runtime(String),
    Agent(String),
    Model(String),
    Tool(String),
    Cancelled(String),
    Stream(String),
    State(String),
}

impl PyFutureError {
    fn class_name(&self) -> &'static str {
        match self {
            Self::Runtime(_) => "StarweaverError",
            Self::Agent(_) => "AgentError",
            Self::Model(_) => "ModelError",
            Self::Tool(_) => "ToolError",
            Self::Cancelled(_) => "Cancelled",
            Self::Stream(_) => "StreamError",
            Self::State(_) => "StateError",
        }
    }

    fn message(&self) -> &str {
        match self {
            Self::Runtime(message)
            | Self::Agent(message)
            | Self::Model(message)
            | Self::Tool(message)
            | Self::Cancelled(message)
            | Self::Stream(message)
            | Self::State(message) => message,
        }
    }

    pub(crate) fn from_agent_error(error: AgentError) -> Self {
        match error {
            AgentError::Model(error) => Self::Model(error.to_string()),
            AgentError::Cancelled { reason } => Self::Cancelled(reason),
            error @ (AgentError::ToolRetryLimitExceeded { .. }
            | AgentError::ToolCallsRequireTools) => Self::Tool(error.to_string()),
            error @ (AgentError::Capability(_)
            | AgentError::CapabilityOrder(_)
            | AgentError::StructuredOutput(_)
            | AgentError::DynamicInstruction(_)
            | AgentError::OutputRetryLimitExceeded { .. }
            | AgentError::StepLimitExceeded { .. }
            | AgentError::UsageLimit(_)
            | AgentError::ExecutionSuspended { .. }
            | AgentError::Executor(_)) => Self::Agent(error.to_string()),
        }
    }
}

impl From<PyErr> for PyFutureError {
    fn from(error: PyErr) -> Self {
        Self::Runtime(error.to_string())
    }
}

pub(crate) fn spawn_py_future<T, F, C>(py: Python<'_>, future: F, convert: C) -> PyResult<Py<PyAny>>
where
    T: Send + 'static,
    F: Future<Output = Result<T, PyFutureError>> + Send + 'static,
    C: Send + 'static + FnOnce(Python<'_>, T) -> PyResult<Py<PyAny>>,
{
    let asyncio = py.import("asyncio")?;
    let loop_obj = asyncio.call_method0("get_running_loop")?.unbind();
    let py_future = loop_obj.call_method0(py, "create_future")?;
    let py_future_for_task = py_future.clone_ref(py);
    let loop_for_task = loop_obj.clone_ref(py);

    tokio_runtime()?.spawn(async move {
        let output = future.await;
        Python::attach(|py| {
            let schedule_result = match output {
                Ok(value) => match convert(py, value) {
                    Ok(result) => {
                        schedule_future_result(py, &loop_for_task, &py_future_for_task, result)
                    }
                    Err(error) => {
                        let error = PyFutureError::from(error);
                        schedule_future_exception(py, &loop_for_task, &py_future_for_task, &error)
                    }
                },
                Err(error) => {
                    schedule_future_exception(py, &loop_for_task, &py_future_for_task, &error)
                }
            };
            if let Err(error) = schedule_result {
                error.print(py);
            }
        });
    });

    Ok(py_future)
}

fn schedule_future_result(
    py: Python<'_>,
    loop_obj: &Py<PyAny>,
    py_future: &Py<PyAny>,
    result: Py<PyAny>,
) -> PyResult<()> {
    if py_future.call_method0(py, "done")?.extract::<bool>(py)? {
        return Ok(());
    }
    let setter = py_future.getattr(py, "set_result")?;
    loop_obj.call_method1(py, "call_soon_threadsafe", (setter, result))?;
    Ok(())
}

fn schedule_future_exception(
    py: Python<'_>,
    loop_obj: &Py<PyAny>,
    py_future: &Py<PyAny>,
    error: &PyFutureError,
) -> PyResult<()> {
    if py_future.call_method0(py, "done")?.extract::<bool>(py)? {
        return Ok(());
    }
    let setter = py_future.getattr(py, "set_exception")?;
    let exception = match py
        .import("starweaver.errors")
        .and_then(|module| module.getattr(error.class_name()))
    {
        Ok(error_class) => error_class.call1((error.message(),))?.unbind(),
        Err(_) => py
            .import("builtins")?
            .getattr("RuntimeError")?
            .call1((error.message(),))?
            .unbind(),
    };
    loop_obj.call_method1(py, "call_soon_threadsafe", (setter, exception))?;
    Ok(())
}
