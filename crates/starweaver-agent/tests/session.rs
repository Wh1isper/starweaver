#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use starweaver_agent::{AgentBuilder, AgentContext, AgentSession, AgentStreamEvent, FunctionModel};
use starweaver_core::{AgentId, Usage};
use starweaver_model::ModelResponse;

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

    let state = session.export_state();
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
