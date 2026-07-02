//! Native extension module for the Starweaver Python package.

use pyo3::prelude::*;

mod agent;
mod capability;
mod context;
mod conversion;
mod errors;
mod model;
mod output;
mod runtime;
mod stream;
mod subagent;
mod testing;
mod tool;

/// Return the native package version.
#[pyfunction]
fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Starweaver native Python module.
#[pymodule]
fn _native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", version())?;
    m.add_function(wrap_pyfunction!(version, m)?)?;
    m.add_class::<agent::PyAgent>()?;
    m.add_class::<agent::PyAgentStream>()?;
    m.add_class::<agent::PyRunResult>()?;
    m.add_class::<agent::PySession>()?;
    m.add_class::<agent::PyStreamRunResult>()?;
    m.add_class::<capability::PyCapabilityBundle>()?;
    m.add_class::<context::PyToolContext>()?;
    m.add_class::<model::PyModelSettings>()?;
    m.add_class::<model::PyProviderModel>()?;
    m.add_class::<model::PyRequestParams>()?;
    m.add_class::<output::PyOutputContext>()?;
    m.add_class::<output::PyOutputFunction>()?;
    m.add_class::<output::PyOutputPolicy>()?;
    m.add_class::<output::PyOutputSchema>()?;
    m.add_class::<output::PyOutputValidator>()?;
    m.add_class::<output::PyOutputValue>()?;
    m.add_class::<stream::PyStreamEvent>()?;
    m.add_class::<subagent::PySubagent>()?;
    m.add_class::<testing::PyFunctionModel>()?;
    m.add_class::<testing::PyTestModel>()?;
    m.add_class::<tool::PyPythonTool>()?;
    m.add_class::<tool::PyToolResult>()?;
    m.add_function(wrap_pyfunction!(testing::sleep_echo, m)?)?;
    Ok(())
}
