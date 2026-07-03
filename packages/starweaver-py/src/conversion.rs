//! Python and JSON conversion helpers.

use pyo3::{
    exceptions::{PyRuntimeError, PyValueError},
    prelude::*,
};
use serde::Serialize;
use serde_json::{Map, Value};
use starweaver_core::Metadata;

pub(crate) fn py_to_json(py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<Value> {
    if value.is_none() {
        return Ok(Value::Null);
    }
    if let Ok(model_dump_json) = value.getattr("model_dump_json") {
        let text: String = model_dump_json.call0()?.extract()?;
        return serde_json::from_str(&text).map_err(|error| {
            PyValueError::new_err(format!("value is not JSON serializable: {error}"))
        });
    }
    if let Ok(model_dump) = value.getattr("model_dump") {
        let dumped = model_dump.call0()?;
        return py_to_json(py, &dumped);
    }
    let json = py.import("json")?;
    let text: String = json.call_method1("dumps", (value,))?.extract()?;
    serde_json::from_str(&text)
        .map_err(|error| PyValueError::new_err(format!("value is not JSON serializable: {error}")))
}

pub(crate) fn json_to_py(py: Python<'_>, value: &Value) -> PyResult<Py<PyAny>> {
    let json = py.import("json")?;
    Ok(json.call_method1("loads", (value.to_string(),))?.unbind())
}

pub(crate) fn serialize_to_py<T>(py: Python<'_>, value: &T) -> PyResult<Py<PyAny>>
where
    T: Serialize,
{
    let value = serde_json::to_value(value)
        .map_err(|error| PyRuntimeError::new_err(format!("failed to serialize value: {error}")))?;
    json_to_py(py, &value)
}

pub(crate) fn py_to_metadata(py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<Metadata> {
    match py_to_json(py, value)? {
        Value::Object(object) => Ok(object),
        Value::Null => Ok(Map::new()),
        _ => Err(PyValueError::new_err("metadata must be a JSON object")),
    }
}

pub(crate) fn optional_py_to_metadata(
    py: Python<'_>,
    value: Option<&Bound<'_, PyAny>>,
) -> PyResult<Metadata> {
    match value {
        Some(value) => py_to_metadata(py, value),
        None => Ok(Map::new()),
    }
}

pub(crate) fn optional_py_to_json(
    py: Python<'_>,
    value: Option<&Bound<'_, PyAny>>,
) -> PyResult<Option<Value>> {
    match value {
        Some(value) if !value.is_none() => Ok(Some(py_to_json(py, value)?)),
        Some(_) | None => Ok(None),
    }
}
