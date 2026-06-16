#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::{Arc, Mutex};

use starweaver_agent::{
    AgentBuilder, AgentContext, AgentSession, AgentStreamEvent, FunctionModel, TraceContext,
};
use starweaver_core::AgentId;
use starweaver_model::ModelResponse;
use starweaver_usage::Usage;

fn reusable_text_model(text: &'static str) -> FunctionModel {
    FunctionModel::new(move |_messages, _settings, _info| {
        Ok(ModelResponse {
            usage: Usage {
                requests: 1,
                ..Usage::default()
            },
            ..ModelResponse::text(text)
        })
    })
}

#[tokio::test]
async fn session_keeps_context_across_runs() {
    let app = AgentBuilder::new(Arc::new(reusable_text_model("ok"))).build_app();
    let mut session = app.session();

    let first = session.run("hello").await.unwrap();
    let second = session.run("again").await.unwrap();

    assert_eq!(first.output, "ok");
    assert_eq!(second.output, "ok");
    assert!(session.context().message_history.len() > first.messages.len());
    assert_eq!(session.context().usage.requests, 2);
}

#[tokio::test]
async fn session_exports_and_restores_state() {
    let app = AgentBuilder::new(Arc::new(reusable_text_model("ok"))).build_app();
    let mut session = app.session();
    session.run("hello").await.unwrap();
    session
        .context_mut()
        .state
        .set("preference", serde_json::json!({"language": "Chinese"}));

    let state = session.export_full_state();
    let mut restored = app.session_from_state(state);
    let result = restored.run("again").await.unwrap();

    assert_eq!(result.output, "ok");
    assert_eq!(restored.context().usage.requests, 2);
    assert_eq!(
        restored.context().state.get("preference").unwrap()["language"],
        "Chinese"
    );
}

#[tokio::test]
async fn session_accepts_caller_provided_context() {
    let app = AgentBuilder::new(Arc::new(reusable_text_model("ok"))).build_app();
    let mut context = AgentContext::new(AgentId::from_string("agent-session"));
    context
        .state
        .set("workspace", serde_json::json!({"root": "/repo"}));

    let mut session = app.session_with_context(context);
    let result = session.run("hello").await.unwrap();

    assert_eq!(result.output, "ok");
    assert_eq!(session.context().agent_id.as_str(), "agent-session");
    assert_eq!(
        session.context().state.get("workspace").unwrap()["root"],
        "/repo"
    );
}

#[tokio::test]
async fn session_stream_uses_session_context() {
    let mut session =
        AgentSession::new(AgentBuilder::new(Arc::new(reusable_text_model("streamed"))).build());

    let stream = session.run_stream("hello").await.unwrap();

    assert_eq!(stream.result().output, "streamed");
    assert_eq!(session.context().usage.requests, 1);
    assert!(matches!(
        stream.events()[0].event,
        AgentStreamEvent::RunStart { .. }
    ));
    assert!(matches!(
        stream.events().last().unwrap().event,
        AgentStreamEvent::RunComplete { .. }
    ));
}

#[tokio::test]
async fn session_propagates_trace_context_to_model_requests() {
    let observed = Arc::new(Mutex::new(Vec::<TraceContext>::new()));
    let observed_model = observed.clone();
    let model = FunctionModel::new(move |_messages, _settings, info| {
        observed_model
            .lock()
            .unwrap()
            .push(info.context.trace_context);
        Ok(ModelResponse {
            usage: Usage {
                requests: 1,
                ..Usage::default()
            },
            ..ModelResponse::text("traced")
        })
    });
    let trace_context = TraceContext::from_trace_id("trace-session")
        .with_span_id("span-session")
        .with_parent_span_id("root-span");
    let mut session = AgentSession::new(AgentBuilder::new(Arc::new(model)).build())
        .with_trace_context(trace_context.clone());

    let result = session.run("hello").await.unwrap();

    assert_eq!(result.output, "traced");
    assert_eq!(session.context().trace_context, trace_context);
    assert_eq!(observed.lock().unwrap().as_slice(), &[trace_context]);
}

#[tokio::test]
async fn session_accepts_w3c_trace_parent_header() {
    let mut session =
        AgentSession::new(AgentBuilder::new(Arc::new(reusable_text_model("ok"))).build())
            .with_trace_parent("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01");

    let result = session.run("hello").await.unwrap();

    assert_eq!(result.output, "ok");
    assert_eq!(
        session.context().trace_context.trace_id.as_deref(),
        Some("4bf92f3577b34da6a3ce929d0e0e4736")
    );
    assert_eq!(
        session.context().trace_context.parent_span_id.as_deref(),
        Some("00f067aa0ba902b7")
    );
}

#[test]
fn session_helpers_update_metadata_notes_state_and_bus() {
    let mut session =
        AgentSession::new(AgentBuilder::new(Arc::new(reusable_text_model("ok"))).build());

    session.set_state("workspace", serde_json::json!({"root": "/repo"}));
    session.set_note("language", "Chinese");
    session.enqueue_message("task", serde_json::json!({"id": 1}));
    session.set_metadata("owner", serde_json::json!("sdk"));

    assert_eq!(
        session.context().state.get("workspace").unwrap()["root"],
        "/repo"
    );
    assert_eq!(session.context().notes.get("language"), Some("Chinese"));
    assert_eq!(session.context().messages.len(), 1);
    assert_eq!(session.context().metadata["owner"], "sdk");
}
