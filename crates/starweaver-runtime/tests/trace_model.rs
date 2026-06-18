#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use starweaver_context::AgentContext;
use starweaver_model::{
    ContentPart, FunctionModel, ModelMessage, ModelRequest, ModelRequestPart, ModelResponse,
    ModelSettings, TestModel,
};
use starweaver_runtime::{
    Agent, AgentCapability, AgentRunState, CapabilityResult, InMemoryTraceRecorder, TraceLevel,
    TraceRecorder,
};
use starweaver_usage::Usage;

#[tokio::test]
async fn model_trace_events_capture_canonical_request_stream_and_response() {
    let recorder = Arc::new(InMemoryTraceRecorder::new());
    let response = ModelResponse {
        usage: Usage {
            requests: 1,
            input_tokens: 3,
            cache_write_tokens: 0,
            cache_read_tokens: 0,
            output_tokens: 2,
            total_tokens: 5,
            tool_calls: 0,
        },
        ..ModelResponse::text("ok")
    };
    let agent = Agent::new(Arc::new(TestModel::with_responses(vec![response])))
        .with_model_settings(ModelSettings {
            max_tokens: Some(32),
            temperature: Some(0.1),
            ..ModelSettings::default()
        })
        .with_trace_recorder(recorder.clone());

    let result = agent.run_stream("hello").await.unwrap();
    assert_eq!(result.result.output, "ok");

    let spans = recorder.spans();
    let model_span = spans
        .iter()
        .find(|span| span.name == "gen_ai.inference")
        .unwrap();
    let request_event = model_span
        .events
        .iter()
        .find(|event| event.name == "starweaver.model.request")
        .unwrap();
    assert_eq!(
        request_event.attributes["starweaver.model.message_count"],
        json!(1)
    );
    assert_eq!(
        request_event.attributes["gen_ai.request"]["redacted"],
        json!(true)
    );
    assert!(request_event.attributes["gen_ai.request"]["messages"].is_null());

    assert!(model_span
        .events
        .iter()
        .any(|event| event.name == "starweaver.model.stream_event"));
    let response_event = model_span
        .events
        .iter()
        .find(|event| event.name == "starweaver.model.response")
        .unwrap();
    assert_eq!(
        response_event.attributes["gen_ai.usage.input_tokens"],
        json!(3)
    );
    assert_eq!(
        response_event.attributes["gen_ai.response"]["redacted"],
        json!(true)
    );
    assert!(response_event.attributes["gen_ai.response"]["parts"].is_null());
    assert_eq!(
        response_event.attributes["gen_ai.usage.output_tokens"],
        json!(2)
    );
}

struct KeepLatestMessageCapability;

#[async_trait]
impl AgentCapability for KeepLatestMessageCapability {
    async fn prepare_model_messages_with_context(
        &self,
        _state: &mut AgentRunState,
        _context: &mut AgentContext,
        messages: Vec<ModelMessage>,
    ) -> CapabilityResult<Vec<ModelMessage>> {
        Ok(messages.into_iter().rev().take(1).collect())
    }
}

#[tokio::test]
async fn history_compaction_span_records_message_count_change() {
    let recorder = Arc::new(InMemoryTraceRecorder::new());
    let prior = vec![
        ModelMessage::Request(ModelRequest {
            parts: vec![ModelRequestPart::UserPrompt {
                content: vec![ContentPart::Text {
                    text: "old".to_string(),
                }],
                name: None,
                metadata: serde_json::Map::new(),
            }],
            timestamp: None,
            instructions: None,
            run_id: None,
            conversation_id: None,
            metadata: serde_json::Map::new(),
        }),
        ModelMessage::Response(ModelResponse::text("old answer")),
    ];

    Agent::new(Arc::new(TestModel::with_text("ok")))
        .with_capability(Arc::new(KeepLatestMessageCapability))
        .with_trace_recorder(recorder.clone())
        .run_with_history("new", prior)
        .await
        .unwrap();

    let spans = recorder.spans();
    let compaction = spans
        .iter()
        .find(|span| span.name == "starweaver.history.compaction")
        .unwrap();
    assert_eq!(
        compaction.attributes["starweaver.capability.name"],
        json!("trace_model::KeepLatestMessageCapability")
    );
    assert_eq!(
        compaction.attributes["starweaver.history.messages.before"],
        json!(3)
    );
    assert_eq!(
        compaction.attributes["starweaver.history.messages.after"],
        json!(1)
    );
}

#[tokio::test]
async fn model_request_context_carries_llm_debug_metadata() {
    let model = FunctionModel::new(|_messages, _settings, info| {
        assert_eq!(
            info.context.llm_trace_metadata["debug_layer"],
            json!("llm-request")
        );
        Ok(ModelResponse::text("ok"))
    });
    let context = starweaver_model::ModelRequestContext::new(
        starweaver_core::RunId::from_string("run-debug"),
        starweaver_core::ConversationId::from_string("conv-debug"),
    )
    .with_llm_trace_metadata(serde_json::Map::from_iter([(
        "debug_layer".to_string(),
        json!("llm-request"),
    )]));

    let response = starweaver_model::ModelAdapter::request(
        &model,
        vec![ModelMessage::Request(ModelRequest::user_text("hello"))],
        None,
        starweaver_model::ModelRequestParameters::default(),
        context.clone(),
    )
    .await
    .unwrap();
    assert_eq!(response.text_output(), "ok");
    assert_eq!(
        context.llm_trace_metadata["debug_layer"],
        json!("llm-request")
    );
}

#[test]
fn trace_recorder_object_records_debug_filter_spans() {
    let recorder = InMemoryTraceRecorder::new();
    let dyn_recorder: &dyn TraceRecorder = &recorder;
    let span = dyn_recorder.start_span(
        starweaver_runtime::SpanSpec::new("starweaver.filter.all").debug(),
        &starweaver_core::TraceContext::from_trace_id("trace-filter"),
    );
    dyn_recorder.record_event(
        &span,
        starweaver_runtime::SpanEvent::new("starweaver.filter.snapshot").debug(),
    );
    dyn_recorder.close_span(&span, starweaver_runtime::SpanStatus::Ok);

    let spans = recorder.spans();
    assert_eq!(spans[0].level, TraceLevel::Debug);
    assert_eq!(spans[0].events[0].level, TraceLevel::Debug);
}
