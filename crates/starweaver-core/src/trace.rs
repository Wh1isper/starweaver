use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Metadata;

/// Trace context shared by SDK, runtime, model, service, and observability layers.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct TraceContext {
    /// Trace identifier from an external root trace or local tracer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    /// Current span identifier when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span_id: Option<String>,
    /// Parent span identifier when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_span_id: Option<String>,
    /// W3C trace state or collector-specific propagation state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_state: Option<String>,
    /// Additional low-cardinality trace metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

impl TraceContext {
    /// Create an empty trace context.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a trace context from an external trace id.
    #[must_use]
    pub fn from_trace_id(trace_id: impl Into<String>) -> Self {
        Self {
            trace_id: Some(trace_id.into()),
            ..Self::default()
        }
    }

    /// Create a trace context from a W3C traceparent header.
    #[must_use]
    pub fn from_trace_parent(trace_parent: impl Into<String>) -> Self {
        let trace_parent = trace_parent.into();
        let parts = trace_parent.split('-').collect::<Vec<_>>();
        if parts.len() >= 4 {
            let mut metadata = Metadata::default();
            metadata.insert(
                "trace_flags".to_string(),
                Value::String(parts[3].to_string()),
            );
            Self {
                trace_id: Some(parts[1].to_string()),
                parent_span_id: Some(parts[2].to_string()),
                metadata,
                ..Self::default()
            }
        } else {
            Self::from_trace_id(trace_parent)
        }
    }

    /// Attach a span id.
    #[must_use]
    pub fn with_span_id(mut self, span_id: impl Into<String>) -> Self {
        self.span_id = Some(span_id.into());
        self
    }

    /// Attach a parent span id.
    #[must_use]
    pub fn with_parent_span_id(mut self, parent_span_id: impl Into<String>) -> Self {
        self.parent_span_id = Some(parent_span_id.into());
        self
    }

    /// Attach trace state.
    #[must_use]
    pub fn with_trace_state(mut self, trace_state: impl Into<String>) -> Self {
        self.trace_state = Some(trace_state.into());
        self
    }

    /// Attach metadata.
    #[must_use]
    pub fn with_metadata(mut self, metadata: Metadata) -> Self {
        self.metadata = metadata;
        self
    }

    /// Return whether the trace context is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.trace_id.is_none()
            && self.span_id.is_none()
            && self.parent_span_id.is_none()
            && self.trace_state.is_none()
            && self.metadata.is_empty()
    }
}
