//! Python wrappers for Starweaver toolsets.

use std::sync::Arc;

use pyo3::{exceptions::PyValueError, prelude::*};
use starweaver_tools::{
    DynToolset, StaticToolset, ToolInstruction, ToolProxyToolset, ToolSearchToolset,
};

use crate::{
    conversion::serialize_to_py,
    tool::{PyPythonTool, py_tool_list_to_dyn_tools},
};

/// Python wrapper around a Starweaver toolset.
#[pyclass(name = "Toolset", skip_from_py_object)]
#[derive(Clone)]
pub struct PyToolset {
    inner: DynToolset,
}

impl PyToolset {
    pub(crate) fn new(inner: DynToolset) -> Self {
        Self { inner }
    }

    pub(crate) fn dyn_toolset(&self) -> DynToolset {
        self.inner.clone()
    }
}

#[pymethods]
impl PyToolset {
    #[new]
    #[pyo3(signature = (name, tools=None, instructions=None, id=None, max_retries=None, timeout_ms=None))]
    fn new_py(
        py: Python<'_>,
        name: String,
        tools: Option<Vec<Py<PyPythonTool>>>,
        instructions: Option<Vec<String>>,
        id: Option<String>,
        max_retries: Option<usize>,
        timeout_ms: Option<u64>,
    ) -> PyResult<Self> {
        let mut toolset = StaticToolset::new(name.clone());
        if let Some(id) = id {
            toolset = toolset.with_id(id);
        }
        for tool in py_tool_list_to_dyn_tools(py, tools)? {
            toolset = toolset.with_tool(tool);
        }
        for instruction in instructions.unwrap_or_default() {
            toolset = toolset.with_instruction(ToolInstruction::new(name.clone(), instruction));
        }
        if let Some(max_retries) = max_retries {
            toolset = toolset.with_max_retries(max_retries);
        }
        if let Some(timeout_ms) = timeout_ms {
            toolset = toolset.with_timeout_ms(timeout_ms);
        }
        Ok(Self::new(Arc::new(toolset)))
    }

    #[getter]
    fn name(&self) -> String {
        self.inner.name().to_string()
    }

    #[getter]
    fn id(&self) -> Option<String> {
        self.inner.id().map(ToOwned::to_owned)
    }

    fn tool_definitions(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let definitions = self
            .inner
            .get_tools()
            .into_iter()
            .map(|tool| tool.definition())
            .collect::<Vec<_>>();
        serialize_to_py(py, &definitions)
    }

    fn instructions(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        serialize_to_py(py, &self.inner.get_instructions())
    }
}

/// Build a direct dynamic tool-search toolset.
#[pyfunction]
pub fn tool_search_toolset(
    toolsets: Vec<Py<PyToolset>>,
    max_results: Option<usize>,
    py: Python<'_>,
) -> PyResult<PyToolset> {
    let inner_toolsets = py_toolsets_to_dyn_toolsets(py, Some(toolsets))?;
    let mut toolset = ToolSearchToolset::new(inner_toolsets);
    if let Some(max_results) = max_results {
        toolset = toolset.with_max_results(max_results);
    }
    Ok(PyToolset::new(Arc::new(toolset)))
}

/// Build a fixed two-tool proxy over wrapped toolsets.
#[pyfunction]
pub fn tool_proxy_toolset(
    toolsets: Vec<Py<PyToolset>>,
    prefix: Option<String>,
    max_results: Option<usize>,
    py: Python<'_>,
) -> PyResult<PyToolset> {
    let inner_toolsets = py_toolsets_to_dyn_toolsets(py, Some(toolsets))?;
    let mut toolset = ToolProxyToolset::new(inner_toolsets);
    if let Some(prefix) = prefix {
        toolset = toolset
            .try_with_name_prefix(prefix)
            .map_err(|error| PyValueError::new_err(error.to_string()))?;
    }
    if let Some(max_results) = max_results {
        toolset = toolset.with_max_results(max_results);
    }
    Ok(PyToolset::new(Arc::new(toolset)))
}

/// Build the first-party filesystem toolset backed by an attached environment.
#[pyfunction]
pub fn filesystem_toolset() -> PyToolset {
    PyToolset::new(starweaver_agent::filesystem_tools())
}

/// Build the first-party foreground shell toolset backed by an attached environment.
#[pyfunction]
pub fn shell_toolset() -> PyToolset {
    PyToolset::new(starweaver_agent::shell_tools())
}

/// Build the first-party filesystem and shell toolsets.
#[pyfunction]
pub fn environment_toolsets() -> Vec<PyToolset> {
    starweaver_agent::environment_toolsets()
        .into_iter()
        .map(PyToolset::new)
        .collect()
}

pub(crate) fn py_toolsets_to_dyn_toolsets(
    py: Python<'_>,
    toolsets: Option<Vec<Py<PyToolset>>>,
) -> PyResult<Vec<DynToolset>> {
    let mut result = Vec::new();
    for toolset in toolsets.unwrap_or_default() {
        result.push(toolset.borrow(py).dyn_toolset());
    }
    Ok(result)
}
