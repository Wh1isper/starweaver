//! Native extension module for the Starweaver Python package.

use pyo3::prelude::*;

mod agent;
mod capability;
mod context;
mod conversion;
mod environment;
mod errors;
mod media;
mod model;
mod output;
mod runtime;
mod skills;
mod store;
mod stream;
mod subagent;
mod testing;
mod tool;
mod toolset;

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
    m.add_class::<agent::PyAgentRuntime>()?;
    m.add_class::<agent::PyAgentStream>()?;
    m.add_class::<agent::PyRunResult>()?;
    m.add_class::<agent::PySession>()?;
    m.add_class::<agent::PyStreamRunResult>()?;
    m.add_class::<capability::PyCapabilityBundle>()?;
    m.add_class::<context::PyToolContext>()?;
    m.add_class::<context::PyToolsetContext>()?;
    m.add_class::<environment::PyEnvironmentProvider>()?;
    m.add_class::<media::PyMediaUploader>()?;
    m.add_class::<model::PyModelSettings>()?;
    m.add_class::<model::PyProviderModel>()?;
    m.add_class::<model::PyRequestParams>()?;
    m.add_function(wrap_pyfunction!(model::oauth_provider_status, m)?)?;
    m.add_function(wrap_pyfunction!(model::oauth_provider_redacted_record, m)?)?;
    m.add_class::<output::PyOutputContext>()?;
    m.add_class::<output::PyOutputFunction>()?;
    m.add_class::<output::PyOutputPolicy>()?;
    m.add_class::<output::PyOutputSchema>()?;
    m.add_class::<output::PyOutputValidator>()?;
    m.add_class::<output::PyOutputValue>()?;
    m.add_class::<stream::PyStreamEvent>()?;
    m.add_class::<skills::PySkillPackage>()?;
    m.add_class::<skills::PySkillRegistry>()?;
    m.add_class::<store::PyPythonSessionStore>()?;
    m.add_class::<store::PySqliteReplayEventLog>()?;
    m.add_class::<store::PySqliteSessionStore>()?;
    m.add_class::<store::PySqliteStreamArchive>()?;
    m.add_class::<subagent::PySubagent>()?;
    m.add_class::<testing::PyFunctionModel>()?;
    m.add_class::<testing::PyTestModel>()?;
    m.add_class::<tool::PyPythonTool>()?;
    m.add_class::<tool::PyToolResult>()?;
    m.add_class::<toolset::PyToolsetLifecyclePolicy>()?;
    m.add_class::<toolset::PyToolset>()?;
    m.add_function(wrap_pyfunction!(toolset::environment_toolsets, m)?)?;
    m.add_function(wrap_pyfunction!(toolset::filesystem_toolset, m)?)?;
    m.add_function(wrap_pyfunction!(toolset::shell_toolset, m)?)?;
    m.add_function(wrap_pyfunction!(toolset::tool_search_toolset, m)?)?;
    m.add_function(wrap_pyfunction!(toolset::tool_proxy_toolset, m)?)?;
    m.add_function(wrap_pyfunction!(toolset::combined_toolset, m)?)?;
    m.add_function(wrap_pyfunction!(toolset::mcp_toolset, m)?)?;
    m.add_function(wrap_pyfunction!(toolset::prepared_toolset, m)?)?;
    m.add_function(wrap_pyfunction!(toolset::dynamic_toolset, m)?)?;
    m.add_function(wrap_pyfunction!(toolset::prefixed_toolset, m)?)?;
    m.add_function(wrap_pyfunction!(toolset::filtered_toolset, m)?)?;
    m.add_function(wrap_pyfunction!(toolset::renamed_toolset, m)?)?;
    m.add_function(wrap_pyfunction!(toolset::metadata_toolset, m)?)?;
    m.add_function(wrap_pyfunction!(toolset::approval_required_toolset, m)?)?;
    m.add_function(wrap_pyfunction!(toolset::deferred_toolset, m)?)?;
    m.add_function(wrap_pyfunction!(testing::sleep_echo, m)?)?;
    Ok(())
}
