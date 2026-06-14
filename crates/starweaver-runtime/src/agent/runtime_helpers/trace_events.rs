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
                    serde_json::json!({
                        "messages": messages,
                        "settings": settings,
                        "params": params,
                    }),
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
                .with_attribute("gen_ai.response", serde_json::json!(response))
                .with_attribute(
                    "gen_ai.usage.input_tokens",
                    serde_json::json!(response.usage.input_tokens),
                )
                .with_attribute(
                    "gen_ai.usage.output_tokens",
                    serde_json::json!(response.usage.output_tokens),
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
            SpanEvent::new("starweaver.model.stream_event")
                .with_attribute("gen_ai.response.stream_event", serde_json::json!(event)),
        );
    }
}
