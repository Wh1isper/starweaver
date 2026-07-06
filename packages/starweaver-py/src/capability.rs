//! Python wrappers for SDK capability bundles.

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use starweaver_context::AgentContext;
use starweaver_runtime::{
    AgentCapability, CapabilityBundle, CapabilityError, CapabilityResult, CapabilitySpec,
    StaticCapabilityBundle, run::AgentRunState,
};

use crate::{
    conversion::{py_to_json, serialize_to_py},
    model::{extract_model_settings, extract_request_params},
    output::{PyOutputFunction, PyOutputValidator},
    tool::PyPythonTool,
};

/// Python wrapper around a static Starweaver capability bundle.
#[pyclass(name = "CapabilityBundle", skip_from_py_object)]
#[derive(Clone)]
pub struct PyCapabilityBundle {
    inner: Arc<dyn CapabilityBundle>,
}

impl PyCapabilityBundle {
    pub(crate) fn bundle(&self) -> Arc<dyn CapabilityBundle> {
        self.inner.clone()
    }
}

#[pymethods]
impl PyCapabilityBundle {
    #[new]
    #[pyo3(signature = (name, instructions=None, tools=None, model_settings=None, request_params=None, output_validators=None, output_functions=None, hooks=None))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        py: Python<'_>,
        name: String,
        instructions: Option<Vec<String>>,
        tools: Option<Vec<Py<PyPythonTool>>>,
        model_settings: Option<&Bound<'_, PyAny>>,
        request_params: Option<&Bound<'_, PyAny>>,
        output_validators: Option<Vec<Py<PyOutputValidator>>>,
        output_functions: Option<Vec<Py<PyOutputFunction>>>,
        hooks: Option<Vec<Py<PyPythonCapability>>>,
    ) -> PyResult<Self> {
        let mut bundle = StaticCapabilityBundle::new(name);
        for hook in hooks.unwrap_or_default() {
            bundle = bundle.with_hook(hook.borrow(py).dyn_capability());
        }
        for instruction in instructions.unwrap_or_default() {
            bundle = bundle.with_instruction(instruction);
        }
        for tool in tools.unwrap_or_default() {
            bundle = bundle.with_tool(tool.borrow(py).dyn_tool());
        }
        if let Some(settings) = extract_model_settings(py, model_settings)? {
            bundle = bundle.with_model_settings(settings);
        }
        if let Some(params) = extract_request_params(py, request_params)? {
            bundle = bundle.with_request_params(params);
        }
        for validator in output_validators.unwrap_or_default() {
            bundle = bundle.with_output_validator(validator.borrow(py).dyn_validator());
        }
        for function in output_functions.unwrap_or_default() {
            bundle = bundle.with_output_function(function.borrow(py).dyn_function());
        }
        Ok(Self {
            inner: Arc::new(bundle),
        })
    }
}

/// Python wrapper around a callback-backed runtime capability hook.
#[pyclass(name = "PythonCapability", skip_from_py_object)]
#[derive(Clone)]
pub struct PyPythonCapability {
    inner: Arc<PythonCapabilityHook>,
}

impl PyPythonCapability {
    pub(crate) fn dyn_capability(&self) -> Arc<dyn AgentCapability> {
        self.inner.clone()
    }
}

#[pymethods]
impl PyPythonCapability {
    #[new]
    #[pyo3(signature = (id, on_run_start, event_loop=None))]
    fn new(id: String, on_run_start: Py<PyAny>, event_loop: Option<Py<PyAny>>) -> Self {
        Self {
            inner: Arc::new(PythonCapabilityHook {
                id,
                on_run_start,
                event_loop,
            }),
        }
    }
}

struct PythonCapabilityHook {
    id: String,
    on_run_start: Py<PyAny>,
    event_loop: Option<Py<PyAny>>,
}

unsafe impl Send for PythonCapabilityHook {}
unsafe impl Sync for PythonCapabilityHook {}

#[async_trait]
impl AgentCapability for PythonCapabilityHook {
    fn spec(&self) -> CapabilitySpec {
        CapabilitySpec::new(self.id.clone())
    }

    async fn on_run_start_with_context(
        &self,
        state: &mut AgentRunState,
        _context: &mut AgentContext,
    ) -> CapabilityResult<()> {
        match self.call_on_run_start(state).await {
            Ok(Some(updated)) => {
                *state = updated;
                Ok(())
            }
            Ok(None) => Ok(()),
            Err(error) => Err(CapabilityError::Failed(format!(
                "python capability {} on_run_start failed: {error}",
                self.id
            ))),
        }
    }
}

impl PythonCapabilityHook {
    async fn call_on_run_start(&self, state: &AgentRunState) -> PyResult<Option<AgentRunState>> {
        enum CallbackValue {
            Immediate(Py<PyAny>),
            Future(Py<PyAny>),
        }

        let state_json = serde_json::to_value(state)
            .map_err(|error| pyo3::exceptions::PyValueError::new_err(error.to_string()))?;
        let call_result = Python::attach(|py| -> PyResult<CallbackValue> {
            let state = serialize_to_py(py, &state_json)?;
            let value = self.on_run_start.call1(py, (state,))?;
            let inspect = py.import("inspect")?;
            let is_awaitable = inspect
                .call_method1("isawaitable", (value.bind(py),))?
                .extract::<bool>()?;
            if !is_awaitable {
                return Ok(CallbackValue::Immediate(value));
            }
            let Some(event_loop) = self.event_loop.as_ref() else {
                let _ = value.call_method0(py, "close");
                return Err(PyRuntimeError::new_err(
                    "async PythonCapability on_run_start requires creating the agent inside a running asyncio loop",
                ));
            };
            let asyncio = py.import("asyncio")?;
            let future = asyncio.call_method1(
                "run_coroutine_threadsafe",
                (value, event_loop.clone_ref(py)),
            )?;
            Ok(CallbackValue::Future(future.unbind()))
        })?;

        let value = match call_result {
            CallbackValue::Immediate(value) => value,
            CallbackValue::Future(future) => {
                let guard_future = Python::attach(|py| future.clone_ref(py));
                let mut cancel_guard = PythonCapabilityFutureCancelGuard::new(guard_future);
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
                result?
            }
        };

        Python::attach(|py| {
            if value.is_none(py) {
                return Ok(None);
            }
            serde_json::from_value(py_to_json(py, value.bind(py))?)
                .map(Some)
                .map_err(|error| {
                    pyo3::exceptions::PyValueError::new_err(format!(
                        "invalid PythonCapability on_run_start state: {error}"
                    ))
                })
        })
    }
}

struct PythonCapabilityFutureCancelGuard {
    future: Option<Py<PyAny>>,
}

impl PythonCapabilityFutureCancelGuard {
    fn new(future: Py<PyAny>) -> Self {
        Self {
            future: Some(future),
        }
    }

    fn complete(&mut self) {
        self.future = None;
    }
}

impl Drop for PythonCapabilityFutureCancelGuard {
    fn drop(&mut self) {
        if let Some(future) = self.future.take() {
            Python::attach(|py| {
                let _ = future.call_method0(py, "cancel");
            });
        }
    }
}
