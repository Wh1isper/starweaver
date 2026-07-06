//! Python stream event projections.

use pyo3::prelude::*;
use serde_json::Value;
use starweaver_runtime::{AgentStreamEvent, AgentStreamRecord};
use starweaver_stream::{DisplayMessage, display_to_agui_event, display_to_agui_jsonl};

use crate::conversion::{json_to_py, py_to_json};

/// Python-friendly stream event with typed kind and raw JSON record.
#[pyclass(name = "StreamEvent", skip_from_py_object)]
#[derive(Clone, Debug)]
pub struct PyStreamEvent {
    kind: String,
    raw: Value,
}

impl PyStreamEvent {
    pub(crate) fn from_record(record: &AgentStreamRecord) -> PyResult<Self> {
        Ok(Self {
            kind: event_kind(&record.event).to_string(),
            raw: record.to_raw_json().map_err(|error| {
                pyo3::exceptions::PyRuntimeError::new_err(format!(
                    "failed to serialize stream record: {error}"
                ))
            })?,
        })
    }
}

/// Convert one canonical display message into an AGUI event.
#[pyfunction]
pub fn display_to_agui(message: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
    let py = message.py();
    let message: DisplayMessage =
        serde_json::from_value(py_to_json(py, message)?).map_err(|error| {
            pyo3::exceptions::PyValueError::new_err(format!("invalid display message: {error}"))
        })?;
    let event = display_to_agui_event(&message);
    json_to_py(
        py,
        &serde_json::to_value(event).map_err(|error| {
            pyo3::exceptions::PyRuntimeError::new_err(format!(
                "failed to serialize AGUI event: {error}"
            ))
        })?,
    )
}

/// Convert canonical display messages into AGUI JSONL.
#[pyfunction(name = "display_to_agui_jsonl")]
pub fn display_to_agui_jsonl_py(messages: &Bound<'_, PyAny>) -> PyResult<String> {
    let py = messages.py();
    let messages: Vec<DisplayMessage> =
        serde_json::from_value(py_to_json(py, messages)?).map_err(|error| {
            pyo3::exceptions::PyValueError::new_err(format!("invalid display messages: {error}"))
        })?;
    display_to_agui_jsonl(&messages).map_err(|error| {
        pyo3::exceptions::PyRuntimeError::new_err(format!(
            "failed to serialize AGUI JSONL: {error}"
        ))
    })
}

#[pymethods]
impl PyStreamEvent {
    #[getter]
    fn kind(&self) -> &str {
        &self.kind
    }

    #[getter]
    fn raw(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        json_to_py(py, &self.raw)
    }
}

fn event_kind(event: &AgentStreamEvent) -> &'static str {
    match event {
        AgentStreamEvent::RunStart { .. } => "run_start",
        AgentStreamEvent::NodeStart { .. } => "node_start",
        AgentStreamEvent::NodeComplete { .. } => "node_complete",
        AgentStreamEvent::Custom { .. } => "custom",
        AgentStreamEvent::ModelRequest { .. } => "model_request",
        AgentStreamEvent::ModelStream { .. } => "model_stream",
        AgentStreamEvent::ModelResponse { .. } => "model_response",
        AgentStreamEvent::Checkpoint { .. } => "checkpoint",
        AgentStreamEvent::Suspended { .. } => "suspended",
        AgentStreamEvent::ToolCall { .. } => "tool_call",
        AgentStreamEvent::ToolReturn { .. } => "tool_return",
        AgentStreamEvent::OutputRetry { .. } => "output_retry",
        AgentStreamEvent::SteeringGuard { .. } => "steering_guard",
        AgentStreamEvent::RunComplete { .. } => "run_complete",
        AgentStreamEvent::RunFailed { .. } => "run_failed",
    }
}
