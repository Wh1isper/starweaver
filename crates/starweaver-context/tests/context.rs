#![allow(missing_docs, clippy::unwrap_used)]

use starweaver_context::{AgentContext, AgentEvent, AgentId, BusMessage, ResumableState};
use starweaver_model::{ModelMessage, ModelRequest};

#[test]
fn context_exports_and_restores_state() {
    let mut context = AgentContext::new(AgentId::from_string("main"));
    context.push_message(ModelMessage::Request(ModelRequest::user_text("hello")));
    context
        .state
        .set("notes", serde_json::json!({"answer": 42}));
    context.enqueue_message(BusMessage::new(
        "steering",
        serde_json::json!({"text": "continue"}),
    ));

    let exported = context.export_state();
    let restored = AgentContext::from_state(exported);

    assert_eq!(restored.agent_id.as_str(), "main");
    assert_eq!(restored.message_history.len(), 1);
    assert_eq!(
        restored.state.get("notes"),
        Some(&serde_json::json!({"answer": 42}))
    );
    assert_eq!(restored.messages.len(), 1);
}

#[test]
fn event_bus_records_and_drains_events() {
    let mut context = AgentContext::default();
    context.publish_event(AgentEvent::new("run_start", serde_json::json!({"step": 0})));
    context.publish_event(AgentEvent::new(
        "run_complete",
        serde_json::json!({"step": 1}),
    ));

    assert_eq!(context.events.events().len(), 2);
    let drained = context.events.drain();
    assert_eq!(drained.len(), 2);
    assert!(context.events.events().is_empty());
}

#[test]
fn resumable_state_default_restores_fresh_context() {
    let context = AgentContext::from_state(ResumableState::default());

    assert_eq!(context.agent_id.as_str(), "main");
    assert!(context.message_history.is_empty());
    assert!(context.messages.is_empty());
}

#[derive(Debug, Eq, PartialEq)]
struct WeatherService {
    city: String,
}

#[test]
fn context_stores_typed_and_named_dependencies() {
    let mut context = AgentContext::default();
    context.insert_dependency(WeatherService {
        city: "Paris".to_string(),
    });
    context.insert_named_dependency("answer", 42_u32);

    assert_eq!(
        context.dependency::<WeatherService>().unwrap().city,
        "Paris"
    );
    assert_eq!(*context.named_dependency::<u32>("answer").unwrap(), 42);
    assert!(context.named_dependency::<String>("answer").is_none());
    assert_eq!(context.dependencies.keys().len(), 2);
}

#[test]
fn dependencies_are_not_serialized_in_resumable_state() {
    let mut context = AgentContext::default();
    context.insert_dependency(WeatherService {
        city: "Paris".to_string(),
    });

    let restored = AgentContext::from_state(context.export_state());

    assert!(restored.dependency::<WeatherService>().is_none());
}
