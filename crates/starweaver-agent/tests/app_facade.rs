#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use starweaver_agent::{AgentBuilder, AgentContext, AgentSession, FunctionModel, TraceContext};
use starweaver_model::ModelResponse;

fn reusable_model() -> FunctionModel {
    FunctionModel::new(|_messages, _settings, _info| Ok(ModelResponse::text("ok")))
}

#[tokio::test]
async fn app_and_session_facades_delegate_to_runtime_agent() {
    let app = AgentBuilder::new(Arc::new(reusable_model())).build_app();

    assert_eq!(app.agent().run("via agent").await.unwrap().output, "ok");

    let mut session = app.session();
    assert_eq!(
        session
            .agent()
            .run("via session agent")
            .await
            .unwrap()
            .output,
        "ok"
    );
    session.set_trace_context(TraceContext::from_trace_id("trace-app-session"));
    assert_eq!(
        session.context().trace_context.trace_id.as_deref(),
        Some("trace-app-session")
    );
}

#[tokio::test]
async fn app_run_helpers_cover_history_context_and_streaming() {
    let app = AgentBuilder::new(Arc::new(reusable_model())).build_app();
    let first = app.run("first").await.unwrap();

    let with_history = app
        .run_with_history("second", first.messages.clone())
        .await
        .unwrap();
    assert_eq!(with_history.output, "ok");
    assert_eq!(with_history.new_messages().len(), 2);

    let mut context = AgentContext::default();
    let with_context = app.run_with_context("ctx", &mut context).await.unwrap();
    assert_eq!(with_context.output, "ok");
    assert_eq!(context.message_history.len(), with_context.messages.len());

    let stream = app.run_stream("stream").await.unwrap();
    assert_eq!(stream.result().output, "ok");
    assert!(!stream.events().is_empty());

    let mut explicit_events = Vec::new();
    let explicit = app
        .run_with_context_and_stream_events("events", &mut context, &mut explicit_events)
        .await
        .unwrap();
    assert_eq!(explicit.output, "ok");
    assert!(!explicit_events.is_empty());
}

#[test]
fn app_builds_sessions_from_context_and_exported_state() {
    let app = AgentBuilder::new(Arc::new(reusable_model())).build_app();
    let mut context = AgentContext::default();
    context.state.set("k", serde_json::json!("v"));

    let session = app.session_with_context(context);
    assert_eq!(
        session.context().state.get("k"),
        Some(&serde_json::json!("v"))
    );

    let restored = app.session_from_state(session.export_full_state());
    assert_eq!(
        restored.context().state.get("k"),
        Some(&serde_json::json!("v"))
    );

    let direct = AgentSession::from_state(app.agent().clone(), restored.export_full_state());
    assert_eq!(
        direct.context().state.get("k"),
        Some(&serde_json::json!("v"))
    );
}
