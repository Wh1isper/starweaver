#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::{Arc, LazyLock, Mutex};

use async_trait::async_trait;
use starweaver_context::{AgentContext, AgentId, BusMessage};
use starweaver_model::{
    ContentPart, ModelAdapter, ModelError, ModelMessage, ModelProfile, ModelRequestContext,
    ModelRequestParameters, ModelRequestPart, ModelResponse, ModelSettings, ProtocolFamily,
    TestModel,
};
use starweaver_runtime::{
    Agent, AgentCapability, AgentRunState, AgentRuntimePolicy, AgentStreamEvent, CapabilityError,
};

struct ContextModel;

#[async_trait]
impl ModelAdapter for ContextModel {
    fn model_name(&self) -> &'static str {
        "context-model"
    }

    fn provider_name(&self) -> Option<&'static str> {
        Some("test")
    }

    fn profile(&self) -> &ModelProfile {
        static PROFILE: LazyLock<ModelProfile> =
            LazyLock::new(|| ModelProfile::for_protocol(ProtocolFamily::OpenAiChatCompletions));
        &PROFILE
    }

    fn default_settings(&self) -> Option<&ModelSettings> {
        None
    }

    async fn request(
        &self,
        _messages: Vec<ModelMessage>,
        _settings: Option<ModelSettings>,
        _params: ModelRequestParameters,
        _context: ModelRequestContext,
    ) -> Result<ModelResponse, ModelError> {
        Ok(ModelResponse::text("context output"))
    }
}

#[tokio::test]
async fn agent_run_updates_context_history_usage_and_events() {
    let mut context = AgentContext::new(AgentId::from_string("main"));
    context.enqueue_message(BusMessage::new(
        "steering",
        serde_json::json!({"text": "keep going"}),
    ));

    let result = Agent::new(Arc::new(ContextModel))
        .run_with_context("hello", &mut context)
        .await
        .unwrap();

    assert_eq!(result.output, "context output");
    assert_eq!(context.message_history.len(), 2);
    assert_eq!(context.events.events().len(), 3);
    assert_eq!(context.events.events()[0].kind, "run_start");
    assert_eq!(context.events.events()[1].kind, "steering_received");
    assert_eq!(context.events.events()[2].kind, "run_complete");
    assert_eq!(context.messages.len(), 0);
    let exported = context.export_state();
    assert_eq!(exported.message_history.len(), 2);
    assert_eq!(exported.agent_id.as_str(), "main");
}

#[tokio::test]
async fn steering_messages_are_drained_into_model_requests_and_stream_ack() {
    let model = Arc::new(TestModel::with_text("ok"));
    let mut context = AgentContext::default();
    context.enqueue_message(BusMessage::new(
        "steering",
        serde_json::json!({"id": "steer_test", "text": "focus on scroll behavior"}),
    ));
    context.enqueue_message(BusMessage::new(
        "other",
        serde_json::json!({"text": "keep queued"}),
    ));

    let mut events = Vec::new();
    Agent::new(model.clone())
        .run_with_context_and_stream_events("hello", &mut context, &mut events)
        .await
        .unwrap();

    let captured = model.captured_messages();
    let request = captured
        .first()
        .and_then(|messages| {
            messages.iter().find_map(|message| match message {
                ModelMessage::Request(request) => Some(request),
                ModelMessage::Response(_) => None,
            })
        })
        .unwrap();
    assert!(request.parts.iter().any(|part| matches!(
        part,
        ModelRequestPart::UserPrompt {
            content,
            name: Some(name),
            ..
        } if name == "steering" && content.iter().any(|part| matches!(
            part,
            ContentPart::Text { text } if text.contains("focus on scroll behavior")
        ))
    )));
    assert_eq!(context.messages.len(), 1);
    let ack = context
        .events
        .events()
        .iter()
        .find(|event| event.kind == "steering_received")
        .unwrap();
    assert_eq!(ack.payload["id"], serde_json::json!("steer_test"));
    assert!(events.iter().any(|record| matches!(
        &record.event,
        AgentStreamEvent::Custom { event }
            if event.kind == "steering_received" && event.payload["id"] == serde_json::json!("steer_test")
    )));
}

#[tokio::test]
async fn pending_steering_guard_retries_before_final_output() {
    let model = Arc::new(TestModel::with_responses(vec![
        ModelResponse::text("first answer"),
        ModelResponse::text("second answer"),
    ]));
    let mut context = AgentContext::default();
    let result = Agent::new(model.clone())
        .with_policy(AgentRuntimePolicy {
            output_retries: 1,
            ..AgentRuntimePolicy::default()
        })
        .with_capability(Arc::new(InjectSteeringOnValidation::default()))
        .run_with_context("hello", &mut context)
        .await
        .unwrap();

    assert_eq!(result.output, "second answer");
    let captured_messages = model.captured_messages();
    assert_eq!(captured_messages.len(), 2);
    let second_request = captured_messages[1]
        .iter()
        .rev()
        .find_map(|message| match message {
            ModelMessage::Request(request) => Some(request),
            ModelMessage::Response(_) => None,
        })
        .unwrap();
    assert!(second_request.parts.iter().any(|part| matches!(
        part,
        ModelRequestPart::Instruction { metadata, .. }
            if metadata.get("starweaver.kind") == Some(&serde_json::json!("steering_guard"))
    )));
    assert!(second_request.parts.iter().any(|part| matches!(
        part,
        ModelRequestPart::UserPrompt { name: Some(name), content, .. }
            if name == "steering" && content.iter().any(|part| matches!(
                part,
                ContentPart::Text { text } if text.contains("late steering")
            ))
    )));
}

#[tokio::test]
async fn pending_steering_guard_streams_control_event_without_output_retry() {
    let model = Arc::new(TestModel::with_responses(vec![
        ModelResponse::text("first answer"),
        ModelResponse::text("second answer"),
    ]));
    let mut context = AgentContext::default();
    let mut events = Vec::new();
    let result = Agent::new(model)
        .with_policy(AgentRuntimePolicy {
            output_retries: 0,
            ..AgentRuntimePolicy::default()
        })
        .with_capability(Arc::new(InjectSteeringOnValidation::default()))
        .run_with_context_and_stream_events("hello", &mut context, &mut events)
        .await
        .unwrap();

    assert_eq!(result.output, "second answer");
    assert!(events
        .iter()
        .any(|record| matches!(record.event, AgentStreamEvent::SteeringGuard { .. })));
    assert!(!events
        .iter()
        .any(|record| matches!(record.event, AgentStreamEvent::OutputRetry { .. })));
}

#[tokio::test]
async fn agent_can_resume_from_exported_context_state() {
    let mut context = AgentContext::new(AgentId::from_string("main"));
    Agent::new(Arc::new(ContextModel))
        .run_with_context("first", &mut context)
        .await
        .unwrap();

    let state = context.export_state();
    let mut restored = AgentContext::from_state(state);
    let result = Agent::new(Arc::new(ContextModel))
        .run_with_context("second", &mut restored)
        .await
        .unwrap();

    assert_eq!(result.history_len, 2);
    assert_eq!(result.new_messages().len(), 2);
    assert_eq!(restored.message_history.len(), 4);
}

#[derive(Default)]
struct InjectSteeringOnValidation {
    injected: Mutex<bool>,
}

#[async_trait]
impl AgentCapability for InjectSteeringOnValidation {
    async fn validate_output_with_context(
        &self,
        _state: &mut AgentRunState,
        context: &mut AgentContext,
        _output: &str,
    ) -> Result<(), CapabilityError> {
        let mut injected = self.injected.lock().unwrap();
        if !*injected {
            context.enqueue_message(BusMessage::new(
                "steering",
                serde_json::json!({"id": "late", "text": "late steering"}),
            ));
            *injected = true;
            drop(injected);
            return Ok(());
        }
        drop(injected);
        Ok(())
    }
}

struct SkipModelCapability;

#[async_trait]
impl AgentCapability for SkipModelCapability {
    async fn before_model_request_with_context(
        &self,
        _state: &mut AgentRunState,
        _context: &mut AgentContext,
        _request: &mut starweaver_model::ModelRequest,
        _settings: &mut Option<ModelSettings>,
    ) -> Result<(), CapabilityError> {
        Err(CapabilityError::SkipModelRequest(Box::new(
            ModelResponse::text("skipped"),
        )))
    }
}

#[tokio::test]
async fn skipped_model_request_retains_steering_without_ack() {
    let model = Arc::new(TestModel::with_text("unused"));
    let mut context = AgentContext::default();
    context.enqueue_message(BusMessage::new(
        "steering",
        serde_json::json!({"text": "apply on real request"}),
    ));

    let mut events = Vec::new();
    let result = Agent::new(model.clone())
        .with_capability(Arc::new(SkipModelCapability))
        .run_with_context_and_stream_events("hello", &mut context, &mut events)
        .await
        .unwrap();

    assert_eq!(result.output, "skipped");
    assert!(model.captured_messages().is_empty());
    assert_eq!(context.messages.len(), 1);
    assert!(!context
        .events
        .events()
        .iter()
        .any(|event| event.kind == "steering_received"));
    assert!(!events.iter().any(|record| matches!(
        &record.event,
        AgentStreamEvent::Custom { event } if event.kind == "steering_received"
    )));
}
