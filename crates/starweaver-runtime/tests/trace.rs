#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use starweaver_model::{tool_call_response, ModelResponse};
use starweaver_runtime::{
    AdapterTraceRecorder, Agent, InMemoryTraceRecorder, SpanKind, SpanStatus, TraceLevel,
};
use starweaver_tools::{FunctionTool, ToolContext, ToolRegistry, ToolResult};

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

#[test]
fn adapter_trace_recorder_provides_exporter_seam() {
    let recorder = AdapterTraceRecorder::new();
    let root = starweaver_core::TraceContext::from_trace_id("trace-adapter");
    let span = starweaver_runtime::TraceRecorder::start_span(
        &recorder,
        starweaver_runtime::SpanSpec::new("gen_ai.invoke_agent"),
        &root,
    );
    starweaver_runtime::TraceRecorder::close_span(&recorder, &span, SpanStatus::Ok);

    let spans = recorder.spans();
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].trace_id, "trace-adapter");
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
