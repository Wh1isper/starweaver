//! Native extension module for the Starweaver Python package.

use pyo3::prelude::*;

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
    Ok(())
}
