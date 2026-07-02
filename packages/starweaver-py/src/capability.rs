//! Python wrappers for SDK capability bundles.

use pyo3::prelude::*;
use starweaver_runtime::{CapabilityBundle, StaticCapabilityBundle};
use std::sync::Arc;

use crate::{
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
    #[pyo3(signature = (name, instructions=None, tools=None, model_settings=None, request_params=None, output_validators=None, output_functions=None))]
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
    ) -> PyResult<Self> {
        let mut bundle = StaticCapabilityBundle::new(name);
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
