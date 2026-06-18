#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use starweaver_core::AgentId;
use starweaver_model::{tool_call_response, FinishReason, ModelResponse};
use starweaver_runtime::{
    export_otel_gen_ai_spans, AdapterTraceRecorder, Agent, InMemoryTraceRecorder, OtelGenAiSpan,
    SpanKind, SpanStatus, TraceLevel,
};
use starweaver_tools::{FunctionTool, ToolContext, ToolRegistry, ToolResult};
use starweaver_usage::Usage;

#[tokio::test]
async fn runtime_records_nested_agent_step_model_and_tool_spans() {
    let recorder = Arc::new(InMemoryTraceRecorder::new());
    let tool = FunctionTool::new(
        "echo",
        Some("Echo".to_string()),
        serde_json::json!({"type": "object"}),
        |_context: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
    );
    let tools = ToolRegistry::new().with_tool(Arc::new(tool));
    let model = starweaver_model::TestModel::with_responses(vec![
        tool_call_response("call-1", "echo", serde_json::json!({"text": "hello"})),
        ModelResponse::text("done"),
    ]);

    let result = Agent::new(Arc::new(model))
        .with_tools(tools)
        .with_trace_recorder(recorder.clone())
        .run("hello")
        .await
        .unwrap();

    assert_eq!(result.output, "done");
    let spans = recorder.spans();
    assert!(spans.iter().all(|span| span.status == SpanStatus::Ok));
    let agent_span = spans
        .iter()
        .find(|span| span.name == "gen_ai.invoke_agent")
        .unwrap();
    let step_spans = spans
        .iter()
        .filter(|span| span.name == "starweaver.loop.step")
        .collect::<Vec<_>>();
    let model_spans = spans
        .iter()
        .filter(|span| span.name == "gen_ai.inference")
        .collect::<Vec<_>>();
    let tool_span = spans
        .iter()
        .find(|span| span.name == "gen_ai.execute_tool")
        .unwrap();
    assert_eq!(step_spans.len(), 2);
    assert_eq!(model_spans.len(), 2);
    assert!(model_spans.iter().all(|span| span.kind == SpanKind::Client));
    assert!(model_spans.iter().all(|span| {
        span.events
            .iter()
            .any(|event| event.name == "starweaver.model.request")
            && span
                .events
                .iter()
                .any(|event| event.name == "starweaver.model.response")
    }));
    assert!(tool_span
        .events
        .iter()
        .any(|event| event.name == "starweaver.tool.call"));
    assert!(tool_span
        .events
        .iter()
        .any(|event| event.name == "starweaver.tool.return"));
    let tool_call_event = tool_span
        .events
        .iter()
        .find(|event| event.name == "starweaver.tool.call")
        .unwrap();
    assert_eq!(
        tool_call_event.attributes["gen_ai.tool.call.arguments"]["redacted"],
        serde_json::json!(true)
    );
    let tool_return_event = tool_span
        .events
        .iter()
        .find(|event| event.name == "starweaver.tool.return")
        .unwrap();
    assert_eq!(
        tool_return_event.attributes["gen_ai.tool.call.result"]["redacted"],
        serde_json::json!(true)
    );
    assert!(step_spans
        .iter()
        .all(|span| span.parent_span_id.as_deref() == Some(agent_span.span_id.as_str())));
    assert!(model_spans.iter().all(|span| {
        step_spans
            .iter()
            .any(|step| span.parent_span_id.as_deref() == Some(step.span_id.as_str()))
    }));
    assert!(step_spans
        .iter()
        .any(|step| tool_span.parent_span_id.as_deref() == Some(step.span_id.as_str())));
    assert!(
        spans
            .iter()
            .filter(|span| span.name == "starweaver.checkpoint")
            .count()
            >= 4
    );
}

#[tokio::test]
async fn otel_gen_ai_export_maps_agent_model_tool_and_usage_fields() {
    let recorder = Arc::new(InMemoryTraceRecorder::new());
    let tool = FunctionTool::new(
        "echo",
        Some("Echo".to_string()),
        serde_json::json!({"type": "object"}),
        |_context: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
    );
    let tools = ToolRegistry::new().with_tool(Arc::new(tool));
    let mut final_response = ModelResponse::text("done");
    final_response.model_name = Some("gpt-test-actual".to_string());
    final_response.finish_reason = Some(FinishReason::Stop);
    final_response.usage = Usage {
        requests: 1,
        input_tokens: 10,
        cache_write_tokens: 2,
        cache_read_tokens: 3,
        output_tokens: 4,
        total_tokens: 17,
        tool_calls: 0,
    };
    let model = starweaver_model::TestModel::with_responses(vec![
        tool_call_response("call-1", "echo", serde_json::json!({"text": "hello"})),
        final_response,
    ])
    .with_model_name("gpt-test-request");

    let result = Agent::new(Arc::new(model))
        .with_agent_identity(AgentId::from_string("agent-main"), "Main Agent")
        .with_tools(tools)
        .with_trace_recorder(recorder.clone())
        .run("hello")
        .await
        .unwrap();

    assert_eq!(result.output, "done");
    let export = export_otel_gen_ai_spans(&recorder.spans());
    assert_agent_otel_export(&export);
    assert_model_otel_export(&export);
    assert_tool_otel_export(&export);
}

fn assert_agent_otel_export(export: &[OtelGenAiSpan]) {
    let agent_span = export
        .iter()
        .find(|span| span.name == "gen_ai.invoke_agent")
        .unwrap();
    assert_eq!(
        agent_span.attributes["gen_ai.operation.name"],
        serde_json::json!("invoke_agent")
    );
    assert_eq!(
        agent_span.attributes["gen_ai.agent.id"],
        serde_json::json!("agent-main")
    );
    assert_eq!(
        agent_span.attributes["gen_ai.agent.name"],
        serde_json::json!("Main Agent")
    );
    assert!(agent_span
        .attributes
        .get("gen_ai.conversation.id")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|value| !value.is_empty()));
}

fn assert_model_otel_export(export: &[OtelGenAiSpan]) {
    let model_span = export
        .iter()
        .find(|span| span.name == "gen_ai.inference")
        .unwrap();
    assert_eq!(model_span.kind, SpanKind::Client);
    assert_eq!(
        model_span.attributes["gen_ai.operation.name"],
        serde_json::json!("chat")
    );
    assert_eq!(
        model_span.attributes["gen_ai.provider.name"],
        serde_json::json!("test")
    );
    assert_eq!(
        model_span.attributes["gen_ai.request.model"],
        serde_json::json!("gpt-test-request")
    );
    assert_eq!(
        model_span.attributes["gen_ai.agent.id"],
        serde_json::json!("agent-main")
    );

    let response_model_span = export
        .iter()
        .filter(|span| span.name == "gen_ai.inference")
        .find(|span| {
            span.attributes.get("gen_ai.usage.input_tokens") == Some(&serde_json::json!(10))
        })
        .unwrap();
    assert_eq!(
        response_model_span.attributes["gen_ai.response.model"],
        serde_json::json!("gpt-test-actual")
    );
    assert_eq!(
        response_model_span.attributes["gen_ai.response.finish_reasons"],
        serde_json::json!(["stop"])
    );
    assert_eq!(
        response_model_span.attributes["gen_ai.usage.input_tokens"],
        serde_json::json!(10)
    );
    assert_eq!(
        response_model_span.attributes["gen_ai.usage.output_tokens"],
        serde_json::json!(4)
    );
    assert_eq!(
        response_model_span.attributes["gen_ai.usage.cache_read.input_tokens"],
        serde_json::json!(3)
    );
    assert_eq!(
        response_model_span.attributes["gen_ai.usage.cache_creation.input_tokens"],
        serde_json::json!(2)
    );
}

fn assert_tool_otel_export(export: &[OtelGenAiSpan]) {
    let tool_span = export
        .iter()
        .find(|span| span.name == "gen_ai.execute_tool")
        .unwrap();
    assert_eq!(
        tool_span.attributes["gen_ai.tool.name"],
        serde_json::json!("echo")
    );
    assert!(tool_span.attributes["gen_ai.tool.call.id"]
        .as_str()
        .is_some_and(|value| value.starts_with("sw-tool-")));
    assert_eq!(
        tool_span.attributes["gen_ai.agent.id"],
        serde_json::json!("agent-main")
    );
}

#[test]
fn adapter_trace_recorder_provides_exporter_seam() {
    let recorder = AdapterTraceRecorder::new();
    let root = starweaver_core::TraceContext::from_trace_id("trace-adapter");
    let span = starweaver_runtime::TraceRecorder::start_span(
        &recorder,
        starweaver_runtime::SpanSpec::new("gen_ai.invoke_agent"),
        &root,
    );
    starweaver_runtime::TraceRecorder::close_span(
        &recorder,
        &span,
        SpanStatus::Error {
            error_type: "test_error".to_string(),
        },
    );

    let spans = recorder.spans();
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].trace_id, "trace-adapter");
    let export = recorder.otel_gen_ai_spans();
    assert_eq!(export.len(), 1);
    assert_eq!(export[0].trace_id, "trace-adapter");
    assert_eq!(
        export[0].attributes["error.type"],
        serde_json::json!("test_error")
    );
}

#[test]
fn span_specs_and_events_carry_trace_levels() {
    let recorder = InMemoryTraceRecorder::new();
    let root = starweaver_core::TraceContext::from_trace_id("trace-debug");
    let span = starweaver_runtime::TraceRecorder::start_span(
        &recorder,
        starweaver_runtime::SpanSpec::new("starweaver.filter.all").debug(),
        &root,
    );
    starweaver_runtime::TraceRecorder::record_event(
        &recorder,
        &span,
        starweaver_runtime::SpanEvent::new("starweaver.filter.snapshot").debug(),
    );
    starweaver_runtime::TraceRecorder::close_span(&recorder, &span, SpanStatus::Ok);

    let spans = recorder.spans();
    assert_eq!(spans[0].level, TraceLevel::Debug);
    assert_eq!(spans[0].events[0].level, TraceLevel::Debug);
}
