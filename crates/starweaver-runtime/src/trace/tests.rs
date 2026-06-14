use starweaver_core::TraceContext;

use super::*;

#[test]
fn in_memory_recorder_nests_child_spans() {
    let recorder = InMemoryTraceRecorder::new();
    let parent = TraceContext::from_trace_id("trace-test").with_span_id("root");
    let agent = recorder.start_span(SpanSpec::new("gen_ai.invoke_agent"), &parent);
    let model = recorder.start_span(SpanSpec::new("gen_ai.inference"), agent.context());
    recorder.close_span(&model, SpanStatus::Ok);
    recorder.close_span(&agent, SpanStatus::Ok);
    let spans = recorder.spans();
    assert_eq!(spans.len(), 2);
    assert_eq!(spans[0].parent_span_id.as_deref(), Some("root"));
    assert_eq!(
        spans[1].parent_span_id.as_deref(),
        spans[0].span_id.as_str().into()
    );
}

#[test]
fn span_specs_and_events_capture_kind_level_and_attributes() {
    let recorder = InMemoryTraceRecorder::new();
    let span = recorder.start_span(
        SpanSpec::new("starweaver.filter.all")
            .with_kind(SpanKind::Client)
            .debug()
            .with_attribute("starweaver.filter.name", serde_json::json!("all")),
        &TraceContext::default(),
    );
    recorder.record_event(
        &span,
        SpanEvent::new("starweaver.filter.snapshot")
            .debug()
            .with_attribute("before", serde_json::json!(2))
            .with_attribute("after", serde_json::json!(1)),
    );
    recorder.close_span(
        &span,
        SpanStatus::Error {
            error_type: "filter_failed".to_string(),
        },
    );

    let spans = recorder.spans();
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].kind, SpanKind::Client);
    assert_eq!(spans[0].level, TraceLevel::Debug);
    assert_eq!(
        spans[0].attributes["starweaver.filter.name"],
        serde_json::json!("all")
    );
    assert_eq!(spans[0].events[0].level, TraceLevel::Debug);
    assert_eq!(
        spans[0].events[0].attributes["before"],
        serde_json::json!(2)
    );
    assert!(matches!(
        spans[0].status,
        SpanStatus::Error { ref error_type } if error_type == "filter_failed"
    ));
}

#[test]
fn no_op_recorder_preserves_parent_context() {
    let recorder = NoopTraceRecorder;
    let parent = TraceContext::from_trace_id("trace-noop").with_span_id("parent");
    let span = recorder.start_span(SpanSpec::new("noop"), &parent);
    recorder.record_event(&span, SpanEvent::new("ignored"));
    recorder.close_span(&span, SpanStatus::Ok);

    assert_eq!(span.span_id(), "parent");
    assert_eq!(span.context().trace_id.as_deref(), Some("trace-noop"));
    assert_eq!(span.into_context().span_id.as_deref(), Some("parent"));
}

#[test]
fn adapter_recorder_delegates_to_in_memory_store() {
    let recorder = AdapterTraceRecorder::new();
    let parent = TraceContext::from_trace_id("trace-adapter");
    let span = recorder.start_span(
        SpanSpec::new("gen_ai.inference").with_kind(SpanKind::Client),
        &parent,
    );
    recorder.record_event(&span, SpanEvent::new("starweaver.model.request"));
    recorder.close_span(&span, SpanStatus::Ok);

    let spans = recorder.spans();
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].trace_id, "trace-adapter");
    assert_eq!(spans[0].kind, SpanKind::Client);
    assert_eq!(spans[0].events[0].name, "starweaver.model.request");
}

#[test]
fn in_memory_recorder_ignores_unknown_span_updates() {
    let recorder = InMemoryTraceRecorder::new();
    let unknown = SpanHandle::new(TraceContext::from_trace_id("trace-missing"), "missing");
    recorder.record_event(&unknown, SpanEvent::new("missing.event"));
    recorder.close_span(&unknown, SpanStatus::Ok);
    assert!(recorder.spans().is_empty());
}
