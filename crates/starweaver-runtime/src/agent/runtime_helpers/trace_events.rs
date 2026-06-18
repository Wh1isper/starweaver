//! Model trace event helpers.

use starweaver_model::{
    ModelMessage, ModelRequestParameters, ModelResponse, ModelResponseStreamEvent, ModelSettings,
};

use crate::{agent::Agent, trace::SpanEvent};

impl Agent {
    pub(in crate::agent) fn record_model_request_event(
        &self,
        span: &crate::trace::SpanHandle,
        messages: &[ModelMessage],
        settings: Option<&ModelSettings>,
        params: &ModelRequestParameters,
    ) {
        self.trace_recorder.record_event(
            span,
            SpanEvent::new("starweaver.model.request")
                .with_attribute(
                    "starweaver.model.message_count",
                    serde_json::json!(messages.len()),
                )
                .with_attribute(
                    "starweaver.model.tool_count",
                    serde_json::json!(params.tools.len()),
                )
                .with_attribute(
                    "starweaver.model.native_tool_count",
                    serde_json::json!(params.native_tools.len()),
                )
                .with_attribute(
                    "starweaver.model.has_output_schema",
                    serde_json::json!(params.output_schema.is_some()),
                )
                .with_attribute(
                    "gen_ai.request",
                    model_request_summary(messages, settings, params),
                ),
        );
    }

    pub(in crate::agent) fn record_model_response_event(
        &self,
        span: &crate::trace::SpanHandle,
        response: &ModelResponse,
    ) {
        self.trace_recorder.record_event(
            span,
            SpanEvent::new("starweaver.model.response")
                .with_attribute("gen_ai.response", model_response_summary(response))
                .with_attribute(
                    "gen_ai.usage.input_tokens",
                    serde_json::json!(response.usage.input_tokens),
                )
                .with_attribute(
                    "gen_ai.usage.output_tokens",
                    serde_json::json!(response.usage.output_tokens),
                )
                .with_attribute(
                    "gen_ai.usage.cache_read.input_tokens",
                    serde_json::json!(response.usage.cache_read_tokens),
                )
                .with_attribute(
                    "gen_ai.usage.cache_creation.input_tokens",
                    serde_json::json!(response.usage.cache_write_tokens),
                ),
        );
    }

    pub(in crate::agent) fn record_model_stream_event(
        &self,
        span: &crate::trace::SpanHandle,
        event: &ModelResponseStreamEvent,
    ) {
        self.trace_recorder.record_event(
            span,
            SpanEvent::new("starweaver.model.stream_event").with_attribute(
                "gen_ai.response.stream_event",
                model_stream_event_summary(event),
            ),
        );
    }
}

fn model_request_summary(
    messages: &[ModelMessage],
    settings: Option<&ModelSettings>,
    params: &ModelRequestParameters,
) -> serde_json::Value {
    serde_json::json!({
        "redacted": true,
        "message_count": messages.len(),
        "tool_names": params.tools.iter().map(|tool| tool.name.clone()).collect::<Vec<_>>(),
        "native_tool_count": params.native_tools.len(),
        "output_schema_name": params.output_schema.as_ref().and_then(|schema| {
            schema.get("name").and_then(serde_json::Value::as_str).map(str::to_string)
        }),
        "settings": settings.map(|settings| serde_json::json!({
            "temperature": settings.temperature,
            "max_tokens": settings.max_tokens,
            "has_thinking": settings.thinking.is_some(),
        })),
    })
}

fn model_response_summary(response: &ModelResponse) -> serde_json::Value {
    let text_chars = response.text_output().chars().count();
    let tool_calls = response
        .tool_calls()
        .into_iter()
        .map(|call| call.name)
        .collect::<Vec<_>>();
    serde_json::json!({
        "redacted": true,
        "part_count": response.parts.len(),
        "text_chars": text_chars,
        "tool_call_names": tool_calls,
        "finish_reason": response.finish_reason.clone(),
        "model_name": response.model_name.clone(),
    })
}

fn model_stream_event_summary(event: &ModelResponseStreamEvent) -> serde_json::Value {
    let kind = format!("{event:?}")
        .split([' ', '('])
        .next()
        .unwrap_or("event")
        .to_string();
    serde_json::json!({
        "redacted": true,
        "kind": kind,
    })
}
