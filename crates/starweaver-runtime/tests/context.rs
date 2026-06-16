#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::{Arc, LazyLock, Mutex};

use async_trait::async_trait;
use serde_json::Map;
use starweaver_context::{AgentContext, AgentId, BusMessage};
use starweaver_model::{
    ContentPart, ModelAdapter, ModelError, ModelMessage, ModelProfile, ModelRequestContext,
    ModelRequestParameters, ModelRequestPart, ModelResponse, ModelResponsePart, ModelSettings,
    ProtocolFamily, TestModel, INSTRUCTION_ORIGIN_METADATA, INSTRUCTION_ORIGIN_RUNTIME_CONTEXT,
};
use starweaver_runtime::{
    Agent, AgentCapability, AgentRunState, AgentRuntimePolicy, AgentStreamEvent, CapabilityError,
    CapabilityOrdering, CapabilityResult, CapabilitySpec, RUNTIME_CONTEXT_CAPABILITY_ID,
};
use starweaver_usage::Usage;

struct ContextModel;

struct CompactContextRecorder {
    previous_state: Arc<Mutex<Option<String>>>,
    original_state: Arc<Mutex<Option<String>>>,
    context_previous: Arc<Mutex<Option<String>>>,
    context_prompt: Arc<Mutex<Option<String>>>,
}

struct EffectivePromptAdapter;

struct AfterRuntimeContextMarker;

struct EffectivePromptRecorder {
    metadata_content: Arc<Mutex<Option<Vec<ContentPart>>>>,
}

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

#[async_trait]
impl AgentCapability for EffectivePromptAdapter {
    async fn before_model_request_with_context(
        &self,
        state: &mut AgentRunState,
        _context: &mut AgentContext,
        request: &mut starweaver_model::ModelRequest,
        _settings: &mut Option<ModelSettings>,
    ) -> Result<(), CapabilityError> {
        if state.run_step != 0 {
            return Ok(());
        }
        let Some(ModelRequestPart::UserPrompt { content, .. }) = request
            .parts
            .iter_mut()
            .find(|part| matches!(part, ModelRequestPart::UserPrompt { .. }))
        else {
            return Ok(());
        };
        *content = vec![
            ContentPart::Text {
                text: "inspect this".to_string(),
            },
            ContentPart::ImageUrl {
                url: "https://example.test/image.png".to_string(),
            },
        ];
        Ok(())
    }
}

#[async_trait]
impl AgentCapability for AfterRuntimeContextMarker {
    fn spec(&self) -> CapabilitySpec {
        CapabilitySpec::new("test.after_runtime_context")
            .with_ordering(CapabilityOrdering::default().after(RUNTIME_CONTEXT_CAPABILITY_ID))
    }

    async fn prepare_model_messages_with_context(
        &self,
        _state: &mut AgentRunState,
        _context: &mut AgentContext,
        mut messages: Vec<ModelMessage>,
    ) -> CapabilityResult<Vec<ModelMessage>> {
        let Some(ModelMessage::Request(request)) = messages
            .iter_mut()
            .rev()
            .find(|message| matches!(message, ModelMessage::Request(_)))
        else {
            return Ok(messages);
        };
        let runtime_context_seen = request.parts.iter().any(|part| {
            matches!(
                part,
                ModelRequestPart::UserPrompt { content, metadata, .. }
                    if metadata.get(INSTRUCTION_ORIGIN_METADATA)
                        == Some(&serde_json::json!(INSTRUCTION_ORIGIN_RUNTIME_CONTEXT))
                        && content.iter().any(|part| matches!(
                            part,
                            ContentPart::Text { text } if text.contains("<runtime-context>")
                        ))
            )
        });
        request.metadata.insert(
            "test_after_runtime_context_seen".to_string(),
            serde_json::json!(runtime_context_seen),
        );
        Ok(messages)
    }
}

#[async_trait]
impl AgentCapability for EffectivePromptRecorder {
    async fn before_model_request_with_context(
        &self,
        _state: &mut AgentRunState,
        _context: &mut AgentContext,
        _request: &mut starweaver_model::ModelRequest,
        _settings: &mut Option<ModelSettings>,
    ) -> Result<(), CapabilityError> {
        Ok(())
    }

    async fn on_run_complete_with_context(
        &self,
        state: &mut AgentRunState,
        _context: &mut AgentContext,
    ) -> CapabilityResult<()> {
        *self.metadata_content.lock().unwrap() = state
            .metadata
            .get("starweaver_original_request_content")
            .and_then(|value| serde_json::from_value::<Vec<ContentPart>>(value.clone()).ok());
        Ok(())
    }
}

#[async_trait]
impl AgentCapability for CompactContextRecorder {
    async fn on_run_start_with_context(
        &self,
        state: &mut AgentRunState,
        context: &mut AgentContext,
    ) -> CapabilityResult<()> {
        *self.previous_state.lock().unwrap() = state
            .metadata
            .get("starweaver_previous_assistant_response_reference")
            .and_then(serde_json::Value::as_str)
            .map(ToString::to_string);
        *self.original_state.lock().unwrap() = state
            .metadata
            .get("starweaver_original_request")
            .and_then(serde_json::Value::as_str)
            .map(ToString::to_string);
        self.context_previous
            .lock()
            .unwrap()
            .clone_from(&context.previous_assistant_response_reference);
        *self.context_prompt.lock().unwrap() = context.user_prompts.as_ref().and_then(|content| {
            content.iter().find_map(|part| match part {
                starweaver_model::ContentPart::Text { text } => Some(text.clone()),
                _ => None,
            })
        });
        Ok(())
    }
}

#[tokio::test]
async fn run_start_captures_compact_restore_context() {
    let previous_state_value = Arc::new(Mutex::new(None));
    let original_state_value = Arc::new(Mutex::new(None));
    let context_previous_value = Arc::new(Mutex::new(None));
    let context_prompt_value = Arc::new(Mutex::new(None));
    let mut context = AgentContext {
        previous_assistant_response_reference: Some("stale reference".to_string()),
        message_history: vec![
            ModelMessage::Request(starweaver_model::ModelRequest::user_text(
                "What should we do?",
            )),
            ModelMessage::Response(ModelResponse::text("1. Add tests\n2. Update docs")),
            ModelMessage::Response(ModelResponse {
                parts: vec![ModelResponsePart::Thinking {
                    text: "private reasoning".to_string(),
                    signature: None,
                }],
                usage: Usage::default(),
                model_name: None,
                provider: None,
                finish_reason: None,
                timestamp: None,
                run_id: None,
                conversation_id: None,
                metadata: Map::default(),
            }),
        ],
        ..AgentContext::default()
    };

    Agent::new(Arc::new(ContextModel))
        .with_capability(Arc::new(CompactContextRecorder {
            previous_state: previous_state_value.clone(),
            original_state: original_state_value.clone(),
            context_previous: context_previous_value.clone(),
            context_prompt: context_prompt_value.clone(),
        }))
        .run_with_context("do 1 and 2", &mut context)
        .await
        .unwrap();

    assert_eq!(
        previous_state_value.lock().unwrap().as_deref(),
        Some("1. Add tests\n2. Update docs"),
    );
    assert_eq!(
        original_state_value.lock().unwrap().as_deref(),
        Some("do 1 and 2"),
    );
    assert_eq!(
        context_previous_value.lock().unwrap().as_deref(),
        Some("1. Add tests\n2. Update docs"),
    );
    assert_eq!(
        context_prompt_value.lock().unwrap().as_deref(),
        Some("do 1 and 2"),
    );
    assert_eq!(
        context.previous_assistant_response_reference.as_deref(),
        Some("1. Add tests\n2. Update docs"),
    );
}

#[tokio::test]
async fn before_request_rewrites_update_compact_restore_user_prompt() {
    let metadata_content = Arc::new(Mutex::new(None));
    let mut context = AgentContext::default();

    Agent::new(Arc::new(ContextModel))
        .with_capability(Arc::new(EffectivePromptAdapter))
        .with_capability(Arc::new(EffectivePromptRecorder {
            metadata_content: metadata_content.clone(),
        }))
        .run_with_context("raw placeholder", &mut context)
        .await
        .unwrap();

    let expected = vec![
        ContentPart::Text {
            text: "inspect this".to_string(),
        },
        ContentPart::ImageUrl {
            url: "https://example.test/image.png".to_string(),
        },
    ];
    assert_eq!(context.user_prompts.as_deref(), Some(expected.as_slice()));
    assert_eq!(
        metadata_content.lock().unwrap().as_deref(),
        Some(expected.as_slice()),
    );
}

#[tokio::test]
async fn runtime_context_is_builtin_canonical_capability() {
    let model = Arc::new(TestModel::with_text("ok"));
    let mut context = AgentContext::default();

    let result = Agent::new(model.clone())
        .with_capability(Arc::new(AfterRuntimeContextMarker))
        .run_with_context("hello", &mut context)
        .await
        .unwrap();

    assert_eq!(result.output, "ok");
    let captured = model.captured_messages();
    assert_eq!(captured.len(), 1);
    let request = captured[0]
        .iter()
        .find_map(|message| match message {
            ModelMessage::Request(request) => Some(request),
            ModelMessage::Response(_) => None,
        })
        .unwrap();
    assert_eq!(
        request.metadata.get("test_after_runtime_context_seen"),
        Some(&serde_json::json!(true)),
    );
    assert!(request.parts.iter().any(|part| matches!(
        part,
        ModelRequestPart::UserPrompt { content, metadata, .. }
            if metadata.get(INSTRUCTION_ORIGIN_METADATA)
                == Some(&serde_json::json!(INSTRUCTION_ORIGIN_RUNTIME_CONTEXT))
                && content.iter().any(|part| matches!(
                    part,
                    ContentPart::Text { text } if text.contains("<runtime-context>")
                ))
    )));
    assert_eq!(context.message_history.first(), captured[0].first());
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
    assert_eq!(context.user_prompts.as_ref().unwrap().len(), 1);
    assert_eq!(context.steering_messages, vec!["keep going".to_string()]);
    assert_eq!(context.events.events().len(), 3);
    assert_eq!(context.events.events()[0].kind, "run_start");
    assert_eq!(context.events.events()[1].kind, "steering_received");
    assert_eq!(context.events.events()[2].kind, "run_complete");
    assert_eq!(context.messages.len(), 1);
    assert!(!context.messages.has_pending(context.agent_id.as_str()));
    let exported = context.export_full_state();
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
    assert_eq!(context.messages.len(), 2);
    assert!(context.messages.has_pending(context.agent_id.as_str()));
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

    let state = context.export_full_state();
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
