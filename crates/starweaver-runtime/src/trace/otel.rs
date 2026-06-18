use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{RecordedSpan, SpanKind, SpanStatus, TraceLevel};

/// Deterministic OpenTelemetry `GenAI` span projection.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OtelGenAiSpan {
    /// Span name.
    pub name: String,
    /// Span id.
    pub span_id: String,
    /// Trace id.
    pub trace_id: String,
    /// Optional parent span id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_span_id: Option<String>,
    /// OpenTelemetry span kind.
    #[serde(default)]
    pub kind: SpanKind,
    /// Trace detail level.
    #[serde(default)]
    pub level: TraceLevel,
    /// Export-ready attributes.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attributes: BTreeMap<String, Value>,
    /// Span status.
    pub status: SpanStatus,
}

/// Project recorded spans into a deterministic OpenTelemetry `GenAI` shape.
#[must_use]
pub fn export_otel_gen_ai_spans(spans: &[RecordedSpan]) -> Vec<OtelGenAiSpan> {
    spans
        .iter()
        .map(|span| OtelGenAiSpan {
            name: span.name.clone(),
            span_id: span.span_id.clone(),
            trace_id: span.trace_id.clone(),
            parent_span_id: span.parent_span_id.clone(),
            kind: span.kind,
            level: span.level,
            attributes: export_attributes(span),
            status: span.status.clone(),
        })
        .collect()
}

fn export_attributes(span: &RecordedSpan) -> BTreeMap<String, Value> {
    let mut attributes = span.attributes.clone();
    for event in &span.events {
        if event.name == "starweaver.model.response" {
            merge_model_response_event(&mut attributes, &event.attributes);
        }
    }
    if let SpanStatus::Error { error_type } = &span.status {
        attributes
            .entry("error.type".to_string())
            .or_insert_with(|| Value::String(error_type.clone()));
    }
    attributes
}

fn merge_model_response_event(
    attributes: &mut BTreeMap<String, Value>,
    event_attributes: &BTreeMap<String, Value>,
) {
    for key in [
        "gen_ai.usage.input_tokens",
        "gen_ai.usage.output_tokens",
        "gen_ai.usage.cache_read.input_tokens",
        "gen_ai.usage.cache_creation.input_tokens",
    ] {
        if let Some(value) = event_attributes.get(key) {
            attributes
                .entry(key.to_string())
                .or_insert_with(|| value.clone());
        }
    }

    let Some(response) = event_attributes
        .get("gen_ai.response")
        .and_then(Value::as_object)
    else {
        return;
    };
    if let Some(model_name) = response.get("model_name").and_then(Value::as_str) {
        attributes
            .entry("gen_ai.response.model".to_string())
            .or_insert_with(|| Value::String(model_name.to_string()));
    }
    if let Some(finish_reason) = response
        .get("finish_reason")
        .filter(|value| !value.is_null())
    {
        attributes
            .entry("gen_ai.response.finish_reasons".to_string())
            .or_insert_with(|| Value::Array(vec![finish_reason.clone()]));
    }
}
