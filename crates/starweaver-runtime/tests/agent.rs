#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, LazyLock, Mutex,
};

use async_trait::async_trait;
use starweaver_context::AgentContext;
use starweaver_model::{
    ContentPart, ModelAdapter, ModelError, ModelMessage, ModelRequest, ModelRequestContext,
    ModelRequestParameters, ModelRequestPart, ModelResponse, ModelResponseEventStream,
    ModelResponsePart, ModelResponseStreamEvent, ModelSettings, PartDelta, ToolCallPart,
    ToolReturnPart,
};
use starweaver_runtime::{
    Agent, AgentCapability, AgentError, AgentInput, AgentRuntimePolicy, CapabilityError,
    CapabilityResult, RetryEventKind,
};
use starweaver_usage::Usage;

#[derive(Clone)]
struct ScriptedModel {
    responses: Arc<Mutex<Vec<ModelResponse>>>,
    captured: Arc<Mutex<Vec<Vec<ModelMessage>>>>,
}

impl ScriptedModel {
    fn new(responses: Vec<ModelResponse>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses.into_iter().rev().collect())),
            captured: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

#[derive(Clone)]
struct StreamingOnlyModel {
    response: Arc<Mutex<Option<ModelResponse>>>,
    captured: Arc<Mutex<Vec<Vec<ModelMessage>>>>,
    request_calls: Arc<AtomicUsize>,
    stream_calls: Arc<AtomicUsize>,
}

impl StreamingOnlyModel {
    fn new(output: &str) -> Self {
        Self {
            response: Arc::new(Mutex::new(Some(ModelResponse::text(output)))),
            captured: Arc::new(Mutex::new(Vec::new())),
            request_calls: Arc::new(AtomicUsize::new(0)),
            stream_calls: Arc::new(AtomicUsize::new(0)),
        }
    }
}

#[async_trait]
impl ModelAdapter for StreamingOnlyModel {
    fn model_name(&self) -> &'static str {
        "streaming-only-test-model"
    }

    fn provider_name(&self) -> Option<&str> {
        Some("test")
    }

    fn profile(&self) -> &starweaver_model::ModelProfile {
        static PROFILE: LazyLock<starweaver_model::ModelProfile> = LazyLock::new(|| {
            starweaver_model::ModelProfile::for_protocol(
                starweaver_model::ProtocolFamily::OpenAiChatCompletions,
            )
        });
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
        self.request_calls.fetch_add(1, Ordering::SeqCst);
        Err(ModelError::Transport(
            "non-stream request must not be used".to_string(),
        ))
    }

    async fn request_stream(
        &self,
        messages: Vec<ModelMessage>,
        _settings: Option<ModelSettings>,
        _params: ModelRequestParameters,
        _context: ModelRequestContext,
    ) -> Result<Vec<ModelResponseStreamEvent>, ModelError> {
        self.stream_calls.fetch_add(1, Ordering::SeqCst);
        self.captured.lock().unwrap().push(messages);
        let response = self
            .response
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| ModelError::Transport("stream script exhausted".to_string()))?;
        Ok(vec![ModelResponseStreamEvent::FinalResult(Box::new(
            response,
        ))])
    }
}

#[async_trait]
impl ModelAdapter for ScriptedModel {
    fn model_name(&self) -> &'static str {
        "test-model"
    }

    fn provider_name(&self) -> Option<&str> {
        Some("test")
    }

    fn profile(&self) -> &starweaver_model::ModelProfile {
        static PROFILE: LazyLock<starweaver_model::ModelProfile> = LazyLock::new(|| {
            starweaver_model::ModelProfile::for_protocol(
                starweaver_model::ProtocolFamily::OpenAiChatCompletions,
            )
        });
        &PROFILE
    }

    fn default_settings(&self) -> Option<&ModelSettings> {
        None
    }

    async fn request(
        &self,
        messages: Vec<ModelMessage>,
        _settings: Option<ModelSettings>,
        _params: ModelRequestParameters,
        _context: ModelRequestContext,
    ) -> Result<ModelResponse, ModelError> {
        self.captured.lock().unwrap().push(messages);
        self.responses
            .lock()
            .unwrap()
            .pop()
            .ok_or_else(|| ModelError::Transport("script exhausted".to_string()))
    }
}

#[test]
fn default_runtime_policy_allows_long_sdk_runs() {
    assert_eq!(AgentRuntimePolicy::default().max_steps, 10_000);
}

#[tokio::test]
async fn bare_agent_runs_prompt_and_returns_output() {
    let model = Arc::new(ScriptedModel::new(vec![ModelResponse::text("Paris")]));
    let agent = Agent::new(model.clone()).with_instruction("Answer concisely.");

    let result = agent.run("What is the capital of France?").await.unwrap();

    assert_eq!(result.output, "Paris");
    assert_eq!(result.state.run_step, 1);
    assert_eq!(result.messages.len(), 2);
    let captured_len = model.captured.lock().unwrap().len();
    assert_eq!(captured_len, 1);
}

#[tokio::test]
async fn capability_can_prepare_run_input_before_first_request() {
    let model = Arc::new(ScriptedModel::new(vec![ModelResponse::text("rewritten")]));
    let agent = Agent::new(model.clone()).with_capability(Arc::new(RewriteRunInput));

    let result = agent.run("original prompt").await.unwrap();

    assert_eq!(result.output, "rewritten");
    let captured = model.captured.lock().unwrap().clone();
    assert!(captured[0].iter().any(|message| match message {
        ModelMessage::Request(request) => request.parts.iter().any(|part| {
            matches!(
                part,
                ModelRequestPart::UserPrompt { content, .. }
                    if content.iter().any(|part| {
                        matches!(part, ContentPart::Text { text } if text == "rewritten prompt")
                    })
            )
        }),
        ModelMessage::Response(_) => false,
    }));
}

#[tokio::test]
async fn capability_can_prepare_pending_tool_returns_before_first_request() {
    let model = Arc::new(ScriptedModel::new(vec![ModelResponse::text("resumed")]));
    let agent = Agent::new(model.clone()).with_capability(Arc::new(InjectPendingToolReturn));
    let history = vec![ModelMessage::Response(ModelResponse {
        parts: vec![ModelResponsePart::ToolCall(ToolCallPart {
            id: "call_prepared".to_string(),
            name: "external_worker".to_string(),
            arguments: serde_json::json!({}).into(),
        })],
        ..ModelResponse::text("")
    })];

    let result = agent
        .run_with_history("should not be sent", history)
        .await
        .unwrap();

    assert_eq!(result.output, "resumed");
    let captured = model.captured.lock().unwrap().clone();
    assert!(captured[0].iter().any(|message| match message {
        ModelMessage::Request(request) => request.parts.iter().any(|part| {
            matches!(
                part,
                ModelRequestPart::ToolReturn(tool_return)
                    if tool_return.name == "external_worker"
                        && tool_return.content["status"] == "ready"
            )
        }),
        ModelMessage::Response(_) => false,
    }));
    assert!(!captured[0].iter().any(|message| match message {
        ModelMessage::Request(request) => request
            .parts
            .iter()
            .any(|part| matches!(part, ModelRequestPart::UserPrompt { .. })),
        ModelMessage::Response(_) => false,
    }));
}

#[tokio::test]
async fn bare_agent_uses_streaming_model_request_without_external_stream_sink() {
    let model = Arc::new(StreamingOnlyModel::new("streamed"));
    let agent = Agent::new(model.clone()).with_instruction("Answer concisely.");

    let result = agent.run("Use the streaming path.").await.unwrap();

    assert_eq!(result.output, "streamed");
    assert_eq!(result.state.run_step, 1);
    assert_eq!(model.request_calls.load(Ordering::SeqCst), 0);
    assert_eq!(model.stream_calls.load(Ordering::SeqCst), 1);
    assert_eq!(model.captured.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn step_limit_failure_preserves_context_state_and_failure_event() {
    let model = Arc::new(ScriptedModel::new(vec![
        ModelResponse::text("bad"),
        ModelResponse::text("unreachable"),
    ]));
    let agent = Agent::new(model)
        .with_policy(AgentRuntimePolicy {
            max_steps: 1,
            output_retries: 2,
            ..AgentRuntimePolicy::default()
        })
        .with_capability(Arc::new(RequireGoodOutput));
    let mut context = AgentContext::default();
    let mut events = Vec::new();

    let error = agent
        .run_with_context_and_stream_events("produce good output", &mut context, &mut events)
        .await
        .unwrap_err();

    assert!(matches!(error, AgentError::StepLimitExceeded { steps: 1 }));
    assert_eq!(context.message_history.len(), 2);
    assert!(matches!(
        context.message_history[0],
        ModelMessage::Request(_)
    ));
    assert!(matches!(
        context.message_history[1],
        ModelMessage::Response(_)
    ));
    assert!(context
        .events
        .events()
        .iter()
        .any(|event| event.kind == "run_failed"));
    assert!(events.iter().any(|record| matches!(
        record.event,
        starweaver_runtime::AgentStreamEvent::RunFailed { ref error_kind, .. }
            if error_kind == "step_limit_exceeded"
    )));
}

#[tokio::test]
async fn bare_agent_retries_output_validation() {
    let model = Arc::new(ScriptedModel::new(vec![
        ModelResponse::text("bad"),
        ModelResponse::text("good"),
    ]));
    let agent = Agent::new(model.clone())
        .with_policy(AgentRuntimePolicy {
            max_steps: 4,
            output_retries: 1,
            ..AgentRuntimePolicy::default()
        })
        .with_capability(Arc::new(RequireGoodOutput));

    let result = agent.run("Produce good output").await.unwrap();

    assert_eq!(result.output, "good");
    assert_eq!(result.state.run_step, 2);
    let second_request_history = model.captured.lock().unwrap()[1].clone();
    let last_request = second_request_history.last().unwrap();
    assert!(format!("{last_request:?}").contains("RetryPrompt"));
}

#[tokio::test]
async fn bare_agent_enforces_output_retry_limit() {
    let model = Arc::new(ScriptedModel::new(vec![ModelResponse::text("bad")]));
    let agent = Agent::new(model)
        .with_policy(AgentRuntimePolicy {
            max_steps: 2,
            output_retries: 0,
            ..AgentRuntimePolicy::default()
        })
        .with_capability(Arc::new(RequireGoodOutput));

    let error = agent.run("Produce good output").await.unwrap_err();

    assert!(matches!(
        error,
        AgentError::OutputRetryLimitExceeded { retries: 0 }
    ));
}

#[tokio::test]
async fn capability_hooks_can_mutate_response_and_record_lifecycle() {
    let model = Arc::new(ScriptedModel::new(vec![ModelResponse {
        parts: vec![ModelResponsePart::Text {
            text: "raw".to_string(),
        }],
        usage: Usage {
            requests: 1,
            input_tokens: 1,
            cache_write_tokens: 0,
            cache_read_tokens: 0,
            output_tokens: 1,
            total_tokens: 2,
            tool_calls: 0,
        },
        model_name: None,
        provider: None,
        finish_reason: None,
        timestamp: None,
        run_id: None,
        conversation_id: None,
        metadata: serde_json::Map::new(),
    }]));
    let hook = Arc::new(RewriteAndRecord::default());
    let agent = Agent::new(model).with_capability(hook.clone());

    let result = agent.run("Rewrite output").await.unwrap();

    assert_eq!(result.output, "rewritten");
    let latest_history_response = match result.messages.last() {
        Some(ModelMessage::Response(response)) => response,
        other => panic!("latest canonical message should be the mutated response, got {other:?}"),
    };
    assert_eq!(latest_history_response.text_output(), "rewritten");
    let Some(latest_response) = result.state.latest_response.as_ref() else {
        panic!("latest response should be recorded");
    };
    assert_eq!(latest_response.text_output(), "rewritten");
    assert_eq!(
        hook.events.lock().unwrap().as_slice(),
        ["start", "before", "after", "validate", "complete"]
    );
}

struct RewriteRunInput;

#[async_trait]
impl AgentCapability for RewriteRunInput {
    async fn prepare_run_input(
        &self,
        _state: &mut starweaver_runtime::AgentRunState,
        _input: AgentInput,
    ) -> CapabilityResult<AgentInput> {
        Ok(AgentInput::text("rewritten prompt"))
    }
}

struct InjectPendingToolReturn;

#[async_trait]
impl AgentCapability for InjectPendingToolReturn {
    async fn prepare_run_input_with_context(
        &self,
        state: &mut starweaver_runtime::AgentRunState,
        _context: &mut AgentContext,
        input: AgentInput,
    ) -> CapabilityResult<AgentInput> {
        state.pending_tool_returns.push(ToolReturnPart::new(
            "call_prepared",
            "external_worker",
            serde_json::json!({"status": "ready"}),
        ));
        Ok(input)
    }
}

struct RequireGoodOutput;

#[async_trait]
impl AgentCapability for RequireGoodOutput {
    async fn validate_output(
        &self,
        _state: &mut starweaver_runtime::AgentRunState,
        output: &str,
    ) -> CapabilityResult<()> {
        if output == "good" {
            Ok(())
        } else {
            Err(CapabilityError::ModelRetry(
                "Return exactly good".to_string(),
            ))
        }
    }
}

#[derive(Default)]
struct RewriteAndRecord {
    events: Mutex<Vec<&'static str>>,
}

#[async_trait]
impl AgentCapability for RewriteAndRecord {
    async fn on_run_start(
        &self,
        _state: &mut starweaver_runtime::AgentRunState,
    ) -> CapabilityResult<()> {
        self.events.lock().unwrap().push("start");
        Ok(())
    }

    async fn before_model_request(
        &self,
        _state: &mut starweaver_runtime::AgentRunState,
        _request: &mut starweaver_model::ModelRequest,
        _settings: &mut Option<ModelSettings>,
    ) -> CapabilityResult<()> {
        self.events.lock().unwrap().push("before");
        Ok(())
    }

    async fn after_model_response(
        &self,
        _state: &mut starweaver_runtime::AgentRunState,
        response: &mut ModelResponse,
    ) -> CapabilityResult<()> {
        self.events.lock().unwrap().push("after");
        response.parts = vec![ModelResponsePart::Text {
            text: "rewritten".to_string(),
        }];
        Ok(())
    }

    async fn validate_output(
        &self,
        _state: &mut starweaver_runtime::AgentRunState,
        _output: &str,
    ) -> CapabilityResult<()> {
        self.events.lock().unwrap().push("validate");
        Ok(())
    }

    async fn on_run_complete(
        &self,
        _state: &mut starweaver_runtime::AgentRunState,
    ) -> CapabilityResult<()> {
        self.events.lock().unwrap().push("complete");
        Ok(())
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn model_error_retry_recovers_context_overflow_history() {
    #[derive(Clone)]
    struct ErrorThenResponseModel {
        calls: Arc<AtomicUsize>,
        captured: Arc<Mutex<Vec<Vec<ModelMessage>>>>,
    }

    #[async_trait]
    impl ModelAdapter for ErrorThenResponseModel {
        fn model_name(&self) -> &'static str {
            "error-then-response"
        }

        fn provider_name(&self) -> Option<&str> {
            Some("test")
        }

        fn profile(&self) -> &starweaver_model::ModelProfile {
            static PROFILE: LazyLock<starweaver_model::ModelProfile> = LazyLock::new(|| {
                starweaver_model::ModelProfile::for_protocol(
                    starweaver_model::ProtocolFamily::OpenAiChatCompletions,
                )
            });
            &PROFILE
        }

        fn default_settings(&self) -> Option<&ModelSettings> {
            None
        }

        async fn request(
            &self,
            messages: Vec<ModelMessage>,
            _settings: Option<ModelSettings>,
            _params: ModelRequestParameters,
            _context: ModelRequestContext,
        ) -> Result<ModelResponse, ModelError> {
            self.captured.lock().unwrap().push(messages);
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                Err(ModelError::ProviderStatus {
                    status: 400,
                    body: serde_json::json!({
                        "error": {
                            "message": "This model's maximum context length is 128000 tokens. Please reduce your prompt.",
                            "code": "context_length_exceeded"
                        }
                    }),
                    retryable: false,
                })
            } else {
                Ok(ModelResponse::text("recovered"))
            }
        }
    }

    let model = Arc::new(ErrorThenResponseModel {
        calls: Arc::new(AtomicUsize::new(0)),
        captured: Arc::new(Mutex::new(Vec::new())),
    });
    let history = vec![
        ModelMessage::Response(ModelResponse {
            parts: vec![ModelResponsePart::ToolCall(ToolCallPart {
                id: "call_1".to_string(),
                name: "view".to_string(),
                arguments: serde_json::json!({"path": "large.txt"}).into(),
            })],
            ..ModelResponse::text("")
        }),
        ModelMessage::Request(ModelRequest {
            parts: vec![ModelRequestPart::ToolReturn(ToolReturnPart::new(
                "call_1",
                "view",
                serde_json::json!("A".repeat(2_000)),
            ))],
            timestamp: None,
            instructions: None,
            run_id: None,
            conversation_id: None,
            metadata: serde_json::Map::new(),
        }),
        ModelMessage::Response(ModelResponse::text("processed")),
        ModelMessage::Request(ModelRequest {
            parts: vec![ModelRequestPart::UserPrompt {
                content: vec![
                    ContentPart::Text {
                        text: "inspect media".to_string(),
                    },
                    ContentPart::Binary {
                        data: b"image".to_vec(),
                        media_type: "image/png".to_string(),
                    },
                ],
                name: None,
                metadata: serde_json::Map::new(),
            }],
            timestamp: None,
            instructions: None,
            run_id: None,
            conversation_id: None,
            metadata: serde_json::Map::new(),
        }),
    ];

    let result = Agent::new(model.clone())
        .run_with_history("continue", history)
        .await
        .unwrap();

    assert_eq!(result.output, "recovered");
    assert_eq!(model.calls.load(Ordering::SeqCst), 2);
    let captured = model.captured.lock().unwrap().clone();
    assert_eq!(captured.len(), 2);
    let retried_history = &captured[1];
    assert!(retried_history.iter().any(|message| match message {
        ModelMessage::Request(request) => request.parts.iter().any(|part| match part {
            ModelRequestPart::ToolReturn(tool_return) => tool_return
                .content
                .as_str()
                .is_some_and(|content| content.contains("chars truncated")),
            _ => false,
        }),
        ModelMessage::Response(_) => false,
    }));
    assert!(retried_history.iter().any(|message| match message {
        ModelMessage::Request(request) => request.parts.iter().any(|part| match part {
            ModelRequestPart::UserPrompt { content, .. } => content.iter().any(|part| {
                matches!(part, ContentPart::Text { text } if text.contains("Media content was removed"))
            }),
            _ => false,
        }),
        ModelMessage::Response(_) => false,
    }));
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn model_error_retry_recovers_stream_context_overflow() {
    #[derive(Clone)]
    struct StreamErrorThenResponseModel {
        calls: Arc<AtomicUsize>,
        captured: Arc<Mutex<Vec<Vec<ModelMessage>>>>,
    }

    #[async_trait]
    impl ModelAdapter for StreamErrorThenResponseModel {
        fn model_name(&self) -> &'static str {
            "stream-error-then-response"
        }

        fn provider_name(&self) -> Option<&str> {
            Some("test")
        }

        fn profile(&self) -> &starweaver_model::ModelProfile {
            static PROFILE: LazyLock<starweaver_model::ModelProfile> = LazyLock::new(|| {
                starweaver_model::ModelProfile::for_protocol(
                    starweaver_model::ProtocolFamily::OpenAiChatCompletions,
                )
            });
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
            Err(ModelError::Transport(
                "non-stream request must not be used".to_string(),
            ))
        }

        async fn request_stream_incremental(
            &self,
            messages: Vec<ModelMessage>,
            _settings: Option<ModelSettings>,
            _params: ModelRequestParameters,
            _context: ModelRequestContext,
        ) -> Result<ModelResponseEventStream, ModelError> {
            self.captured.lock().unwrap().push(messages);
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                return Err(ModelError::ProviderStatus {
                    status: 400,
                    body: serde_json::json!({
                        "error": {
                            "message": "maximum context length exceeded; reduce the length of the messages",
                            "code": "context_length_exceeded"
                        }
                    }),
                    retryable: false,
                });
            }
            let (sender, receiver) = tokio::sync::mpsc::channel(1);
            sender
                .send(Ok(ModelResponseStreamEvent::FinalResult(Box::new(
                    ModelResponse::text("stream recovered"),
                ))))
                .await
                .unwrap();
            Ok(ModelResponseEventStream::new(receiver))
        }
    }

    let model = Arc::new(StreamErrorThenResponseModel {
        calls: Arc::new(AtomicUsize::new(0)),
        captured: Arc::new(Mutex::new(Vec::new())),
    });
    let history = vec![
        ModelMessage::Response(ModelResponse {
            parts: vec![ModelResponsePart::ToolCall(ToolCallPart {
                id: "call_latest".to_string(),
                name: "view".to_string(),
                arguments: serde_json::json!({"path": "large.txt"}).into(),
            })],
            ..ModelResponse::text("")
        }),
        ModelMessage::Request(ModelRequest {
            parts: vec![ModelRequestPart::ToolReturn(ToolReturnPart::new(
                "call_latest",
                "view",
                serde_json::json!("B".repeat(2_000)),
            ))],
            timestamp: None,
            instructions: None,
            run_id: None,
            conversation_id: None,
            metadata: serde_json::Map::new(),
        }),
    ];

    let result = Agent::new(model.clone())
        .run_with_history_and_stream_events("continue", history, &mut Vec::new())
        .await
        .unwrap();

    assert_eq!(result.output, "stream recovered");
    assert_eq!(model.calls.load(Ordering::SeqCst), 2);
    let captured = model.captured.lock().unwrap().clone();
    assert_eq!(captured.len(), 2);
    assert!(captured[1].iter().any(|message| match message {
        ModelMessage::Request(request) => request.parts.iter().any(|part| match part {
            ModelRequestPart::ToolReturn(tool_return) => tool_return
                .content
                .as_str()
                .is_some_and(|content| content.contains("chars truncated")),
            _ => false,
        }),
        ModelMessage::Response(_) => false,
    }));
}

#[tokio::test]
async fn provider_stream_transport_error_resumes_incremental_request() {
    #[derive(Clone)]
    struct StreamTransportErrorThenResponseModel {
        calls: Arc<AtomicUsize>,
        captured: Arc<Mutex<Vec<Vec<ModelMessage>>>>,
    }

    #[async_trait]
    impl ModelAdapter for StreamTransportErrorThenResponseModel {
        fn model_name(&self) -> &'static str {
            "stream-transport-error-then-response"
        }

        fn provider_name(&self) -> Option<&str> {
            Some("test")
        }

        fn profile(&self) -> &starweaver_model::ModelProfile {
            static PROFILE: LazyLock<starweaver_model::ModelProfile> = LazyLock::new(|| {
                starweaver_model::ModelProfile::for_protocol(
                    starweaver_model::ProtocolFamily::OpenAiChatCompletions,
                )
            });
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
            Err(ModelError::Transport(
                "non-stream request must not be used".to_string(),
            ))
        }

        async fn request_stream_incremental(
            &self,
            messages: Vec<ModelMessage>,
            _settings: Option<ModelSettings>,
            _params: ModelRequestParameters,
            _context: ModelRequestContext,
        ) -> Result<ModelResponseEventStream, ModelError> {
            self.captured.lock().unwrap().push(messages);
            let call_index = self.calls.fetch_add(1, Ordering::SeqCst);
            let (sender, receiver) = tokio::sync::mpsc::channel(4);
            if call_index == 0 {
                sender
                    .send(Ok(ModelResponseStreamEvent::PartDelta(PartDelta::text(
                        0, "partial",
                    ))))
                    .await
                    .unwrap();
                sender
                    .send(Err(ModelError::Transport(
                        "server-sent event stream disconnected".to_string(),
                    )))
                    .await
                    .unwrap();
            } else {
                sender
                    .send(Ok(ModelResponseStreamEvent::FinalResult(Box::new(
                        ModelResponse::text("stream resumed"),
                    ))))
                    .await
                    .unwrap();
            }
            Ok(ModelResponseEventStream::new(receiver))
        }
    }

    let model = Arc::new(StreamTransportErrorThenResponseModel {
        calls: Arc::new(AtomicUsize::new(0)),
        captured: Arc::new(Mutex::new(Vec::new())),
    });
    let mut context = AgentContext::default();
    let mut events = Vec::new();

    let result = Agent::new(model.clone())
        .run_with_context_and_stream_events("continue", &mut context, &mut events)
        .await
        .unwrap();

    assert_eq!(result.output, "stream resumed");
    assert_eq!(model.calls.load(Ordering::SeqCst), 2);
    assert_eq!(model.captured.lock().unwrap().len(), 2);
    assert!(context
        .events
        .events()
        .iter()
        .any(|event| event.kind == "model_stream_resume"));
    assert!(events.iter().any(|record| matches!(
        &record.event,
        starweaver_runtime::AgentStreamEvent::Custom { event }
            if event.kind == "model_stream_resume"
    )));
}

#[tokio::test]
async fn provider_stream_clean_close_without_final_resumes_incremental_request() {
    #[derive(Clone)]
    struct StreamCloseThenResponseModel {
        calls: Arc<AtomicUsize>,
        captured: Arc<Mutex<Vec<Vec<ModelMessage>>>>,
    }

    #[async_trait]
    impl ModelAdapter for StreamCloseThenResponseModel {
        fn model_name(&self) -> &'static str {
            "stream-close-then-response"
        }

        fn provider_name(&self) -> Option<&str> {
            Some("test")
        }

        fn profile(&self) -> &starweaver_model::ModelProfile {
            static PROFILE: LazyLock<starweaver_model::ModelProfile> = LazyLock::new(|| {
                starweaver_model::ModelProfile::for_protocol(
                    starweaver_model::ProtocolFamily::OpenAiChatCompletions,
                )
            });
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
            Err(ModelError::Transport(
                "non-stream request must not be used".to_string(),
            ))
        }

        async fn request_stream_incremental(
            &self,
            messages: Vec<ModelMessage>,
            _settings: Option<ModelSettings>,
            _params: ModelRequestParameters,
            _context: ModelRequestContext,
        ) -> Result<ModelResponseEventStream, ModelError> {
            self.captured.lock().unwrap().push(messages);
            let call_index = self.calls.fetch_add(1, Ordering::SeqCst);
            let (sender, receiver) = tokio::sync::mpsc::channel(4);
            if call_index == 0 {
                sender
                    .send(Ok(ModelResponseStreamEvent::PartDelta(PartDelta::text(
                        0, "partial",
                    ))))
                    .await
                    .unwrap();
            } else {
                sender
                    .send(Ok(ModelResponseStreamEvent::FinalResult(Box::new(
                        ModelResponse::text("stream resumed after close"),
                    ))))
                    .await
                    .unwrap();
            }
            Ok(ModelResponseEventStream::new(receiver))
        }
    }

    let model = Arc::new(StreamCloseThenResponseModel {
        calls: Arc::new(AtomicUsize::new(0)),
        captured: Arc::new(Mutex::new(Vec::new())),
    });
    let mut context = AgentContext::default();
    let mut events = Vec::new();

    let result = Agent::new(model.clone())
        .run_with_context_and_stream_events("continue", &mut context, &mut events)
        .await
        .unwrap();

    assert_eq!(result.output, "stream resumed after close");
    assert_eq!(model.calls.load(Ordering::SeqCst), 2);
    assert_eq!(model.captured.lock().unwrap().len(), 2);
    assert!(context.events.events().iter().any(|event| {
        event.kind == "model_stream_resume"
            && event
                .payload
                .get("error")
                .is_some_and(|error| error == "model stream ended before final result")
    }));
    assert!(events.iter().any(|record| matches!(
        &record.event,
        starweaver_runtime::AgentStreamEvent::Custom { event }
            if event.kind == "model_stream_resume"
    )));
}

#[tokio::test]
async fn model_error_retry_ignores_unrecognized_errors() {
    #[derive(Clone)]
    struct TransportErrorModel {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl ModelAdapter for TransportErrorModel {
        fn model_name(&self) -> &'static str {
            "transport-error"
        }

        fn provider_name(&self) -> Option<&str> {
            Some("test")
        }

        fn profile(&self) -> &starweaver_model::ModelProfile {
            static PROFILE: LazyLock<starweaver_model::ModelProfile> = LazyLock::new(|| {
                starweaver_model::ModelProfile::for_protocol(
                    starweaver_model::ProtocolFamily::OpenAiChatCompletions,
                )
            });
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
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(ModelError::Transport("network unavailable".to_string()))
        }
    }

    let model = Arc::new(TransportErrorModel {
        calls: Arc::new(AtomicUsize::new(0)),
    });

    let error = Agent::new(model.clone()).run("hello").await.unwrap_err();

    assert!(matches!(error, AgentError::Model(ModelError::Transport(_))));
    assert_eq!(model.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn capability_hooks_observe_retry_and_output_validation_boundaries() {
    let model = Arc::new(ScriptedModel::new(vec![
        ModelResponse::text("bad"),
        ModelResponse::text("good"),
    ]));
    let hook = Arc::new(RetryAndOutputBoundaryRecorder::default());
    let agent = Agent::new(model).with_capability(hook.clone());

    let result = agent.run("Produce accepted output").await.unwrap();

    assert_eq!(result.output, "good");
    assert_eq!(
        hook.events.lock().unwrap().as_slice(),
        [
            "before_output:bad",
            "retry:output:1:Return exactly good",
            "before_output:good",
            "after_output:good",
        ]
    );
}

#[derive(Default)]
struct RetryAndOutputBoundaryRecorder {
    events: Mutex<Vec<String>>,
}

#[async_trait]
impl AgentCapability for RetryAndOutputBoundaryRecorder {
    async fn before_output_validation(
        &self,
        _state: &mut starweaver_runtime::AgentRunState,
        output: &str,
    ) -> CapabilityResult<()> {
        self.events
            .lock()
            .unwrap()
            .push(format!("before_output:{output}"));
        Ok(())
    }

    async fn validate_output(
        &self,
        _state: &mut starweaver_runtime::AgentRunState,
        output: &str,
    ) -> CapabilityResult<()> {
        if output == "good" {
            Ok(())
        } else {
            Err(CapabilityError::ModelRetry(
                "Return exactly good".to_string(),
            ))
        }
    }

    async fn after_output_validation(
        &self,
        _state: &mut starweaver_runtime::AgentRunState,
        output: &str,
    ) -> CapabilityResult<()> {
        self.events
            .lock()
            .unwrap()
            .push(format!("after_output:{output}"));
        Ok(())
    }

    async fn on_retry(
        &self,
        _state: &mut starweaver_runtime::AgentRunState,
        kind: RetryEventKind,
        retries: usize,
        message: &str,
    ) -> CapabilityResult<()> {
        self.events.lock().unwrap().push(format!(
            "retry:{}:{retries}:{message}",
            match kind {
                RetryEventKind::Output => "output",
                RetryEventKind::Tool => "tool",
            }
        ));
        Ok(())
    }
}
