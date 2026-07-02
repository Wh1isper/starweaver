//! Python stream event projections.

use pyo3::prelude::*;
use serde_json::Value;
use starweaver_runtime::{AgentStreamEvent, AgentStreamRecord};

use crate::conversion::json_to_py;

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
