#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::{
    Arc, LazyLock, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use starweaver_context::AgentContext;
use starweaver_model::{
    ContentPart, ModelAdapter, ModelError, ModelMessage, ModelRequest, ModelRequestContext,
    ModelRequestParameters, ModelRequestPart, ModelResponse, ModelResponseEventStream,
    ModelResponsePart, ModelResponseStreamEvent, ModelSettings, PartDelta, StreamDiagnostic,
    ToolCallPart, ToolReturnPart,
};
use starweaver_runtime::{
    Agent, AgentCapability, AgentError, AgentExecutionNode, AgentInput, AgentRuntimePolicy,
    AgentStreamEvent, AgentStreamRecord, CapabilityError, CapabilityResult,
    DEFAULT_MODEL_ERROR_RESUME_PROMPT, InMemoryTraceRecorder, RetryEventKind, SpanStatus,
};
use starweaver_usage::Usage;

fn model_request_steps(events: &[AgentStreamRecord]) -> Vec<usize> {
    events
        .iter()
        .filter_map(|record| match record.event {
            AgentStreamEvent::ModelRequest { step } => Some(step),
            _ => None,
        })
        .collect()
}

fn node_start_steps(events: &[AgentStreamRecord], expected_node: AgentExecutionNode) -> Vec<usize> {
    events
        .iter()
        .filter_map(|record| match record.event {
            AgentStreamEvent::NodeStart { node, step, .. } if node == expected_node => Some(step),
            _ => None,
        })
        .collect()
}

fn assert_model_attempt_steps(events: &[AgentStreamRecord], expected_steps: &[usize]) {
    assert_eq!(model_request_steps(events), expected_steps);
    assert_eq!(
        node_start_steps(events, AgentExecutionNode::PrepareModelRequest),
        expected_steps
    );
    assert_eq!(
        node_start_steps(events, AgentExecutionNode::BeforeModelRequest),
        expected_steps
    );
}

fn retry_prompt_count(messages: &[ModelMessage], expected: &str) -> usize {
    messages
        .iter()
        .filter_map(|message| match message {
            ModelMessage::Request(request) => Some(&request.parts),
            ModelMessage::Response(_) => None,
        })
        .flatten()
        .filter(|part| {
            matches!(
                part,
                ModelRequestPart::RetryPrompt { text, .. } if text == expected
            )
        })
        .count()
}

fn user_prompt_text_count(messages: &[ModelMessage], expected: &str) -> usize {
    messages
        .iter()
        .filter_map(|message| match message {
            ModelMessage::Request(request) => Some(&request.parts),
            ModelMessage::Response(_) => None,
        })
        .flatten()
        .filter_map(|part| match part {
            ModelRequestPart::UserPrompt { content, .. } => Some(content),
            _ => None,
        })
        .flatten()
        .filter(|part| matches!(part, ContentPart::Text { text } if text == expected))
        .count()
}

#[derive(Clone, Debug, PartialEq)]
struct CapturedProviderInput {
    messages: Vec<ModelMessage>,
    settings: Option<ModelSettings>,
    params: ModelRequestParameters,
    context: ModelRequestContext,
}

#[derive(Default)]
struct StreamRecordObserver {
    records: Mutex<Vec<AgentStreamRecord>>,
}

#[async_trait]
impl AgentCapability for StreamRecordObserver {
    async fn on_stream_event(
        &self,
        _state: &starweaver_runtime::AgentRunState,
        event: &AgentStreamRecord,
    ) -> CapabilityResult<()> {
        self.records.lock().unwrap().push(event.clone());
        Ok(())
    }
}

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
    captured: Arc<Mutex<Vec<CapturedProviderInput>>>,
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
        settings: Option<ModelSettings>,
        params: ModelRequestParameters,
        context: ModelRequestContext,
    ) -> Result<Vec<ModelResponseStreamEvent>, ModelError> {
        self.stream_calls.fetch_add(1, Ordering::SeqCst);
        self.captured.lock().unwrap().push(CapturedProviderInput {
            messages,
            settings,
            params,
            context,
        });
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
    assert!(!captured[0].iter().any(|message| {
        match message {
            ModelMessage::Request(request) => request
                .parts
                .iter()
                .any(|part| matches!(part, ModelRequestPart::UserPrompt { .. })),
            ModelMessage::Response(_) => false,
        }
    }));
}

#[tokio::test]
async fn bare_agent_uses_streaming_model_request_without_external_stream_sink() {
    let model = Arc::new(StreamingOnlyModel::new("streamed"));
    let agent = Agent::new(model.clone())
        .with_instruction("Answer concisely.")
        .with_model_settings(ModelSettings {
            temperature: Some(0.2),
            ..ModelSettings::default()
        })
        .with_request_params(ModelRequestParameters {
            metadata: serde_json::Map::from_iter([(
                "final_only".to_string(),
                serde_json::json!(true),
            )]),
            ..ModelRequestParameters::default()
        });

    let result = agent.run("Use the streaming path.").await.unwrap();

    assert_eq!(result.output, "streamed");
    assert_eq!(result.state.run_step, 1);
    assert_eq!(model.request_calls.load(Ordering::SeqCst), 0);
    assert_eq!(model.stream_calls.load(Ordering::SeqCst), 1);
    let captured = model.captured.lock().unwrap().clone();
    assert_eq!(captured.len(), 1);
    assert_eq!(
        captured[0].settings.as_ref().unwrap().temperature,
        Some(0.2)
    );
    assert_eq!(
        captured[0].params.metadata["final_only"],
        serde_json::json!(true)
    );
    assert_eq!(captured[0].context.run_id, result.state.run_id);
    assert_eq!(
        captured[0].context.conversation_id,
        result.state.conversation_id
    );
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
    assert!(
        context
            .events
            .events()
            .iter()
            .any(|event| event.kind == "run_failed")
    );
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
async fn capability_hooks_mutate_response_before_classification_and_record_lifecycle() {
    let model = Arc::new(ScriptedModel::new(vec![ModelResponse {
        parts: vec![ModelResponsePart::ToolCall(ToolCallPart {
            id: "call_rewritten".to_string(),
            name: "missing_tool".to_string(),
            arguments: serde_json::json!({}).into(),
        })],
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
    assert_eq!(user_prompt_text_count(retried_history, "continue"), 1);
    assert_eq!(
        retry_prompt_count(retried_history, DEFAULT_MODEL_ERROR_RESUME_PROMPT),
        1
    );
    assert!(retried_history.iter().any(|message| match message {
        ModelMessage::Request(request) => request.parts.iter().any(|part| {
            match part {
                ModelRequestPart::ToolReturn(tool_return) => tool_return
                    .content
                    .as_str()
                    .is_some_and(|content| content.contains("chars truncated")),
                _ => false,
            }
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

    let mut events = Vec::new();
    let result = Agent::new(model.clone())
        .run_with_history_and_stream_events("continue", history, &mut events)
        .await
        .unwrap();

    assert_eq!(result.output, "stream recovered");
    assert_eq!(result.state.run_step, 1);
    assert_eq!(model.calls.load(Ordering::SeqCst), 2);
    assert_model_attempt_steps(&events, &[0, 0]);
    let captured = model.captured.lock().unwrap().clone();
    assert_eq!(captured.len(), 2);
    assert_eq!(user_prompt_text_count(&captured[1], "continue"), 1);
    assert_eq!(
        retry_prompt_count(&captured[1], DEFAULT_MODEL_ERROR_RESUME_PROMPT),
        1
    );
    assert!(captured[1].iter().any(|message| match message {
        ModelMessage::Request(request) => request.parts.iter().any(|part| {
            match part {
                ModelRequestPart::ToolReturn(tool_return) => tool_return
                    .content
                    .as_str()
                    .is_some_and(|content| content.contains("chars truncated")),
                _ => false,
            }
        }),
        ModelMessage::Response(_) => false,
    }));
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn provider_stream_request_start_error_resumes_same_prepared_input() {
    #[derive(Clone)]
    struct StartErrorThenResponseModel {
        calls: Arc<AtomicUsize>,
        captured: Arc<Mutex<Vec<CapturedProviderInput>>>,
    }

    #[async_trait]
    impl ModelAdapter for StartErrorThenResponseModel {
        fn model_name(&self) -> &'static str {
            "stream-start-error-then-response"
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
            settings: Option<ModelSettings>,
            params: ModelRequestParameters,
            context: ModelRequestContext,
        ) -> Result<ModelResponseEventStream, ModelError> {
            self.captured.lock().unwrap().push(CapturedProviderInput {
                messages,
                settings,
                params,
                context,
            });
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                return Err(ModelError::Transport(
                    "provider disconnected before stream start".to_string(),
                ));
            }
            let (sender, receiver) = tokio::sync::mpsc::channel(1);
            sender
                .send(Ok(ModelResponseStreamEvent::FinalResult(Box::new(
                    ModelResponse::text("stream resumed after start error"),
                ))))
                .await
                .unwrap();
            Ok(ModelResponseEventStream::new(receiver))
        }
    }

    let model = Arc::new(StartErrorThenResponseModel {
        calls: Arc::new(AtomicUsize::new(0)),
        captured: Arc::new(Mutex::new(Vec::new())),
    });
    let mut context = AgentContext::default();
    let mut events = Vec::new();

    let result = Agent::new(model.clone())
        .run_with_context_and_stream_events("continue", &mut context, &mut events)
        .await
        .unwrap();

    assert_eq!(result.output, "stream resumed after start error");
    assert_eq!(result.state.run_step, 1);
    assert_eq!(model.calls.load(Ordering::SeqCst), 2);
    let captured = model.captured.lock().unwrap().clone();
    assert_eq!(captured.len(), 2);
    assert_eq!(captured[0], captured[1]);
    assert_model_attempt_steps(&events, &[0]);
    let resumes = context
        .events
        .events()
        .iter()
        .filter(|event| event.kind == "model_stream_resume")
        .collect::<Vec<_>>();
    assert_eq!(resumes.len(), 1);
    assert_eq!(resumes[0].payload["retry"], serde_json::json!(1));
    assert_eq!(
        resumes[0].payload["error"],
        serde_json::json!("model transport failed")
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn provider_stream_resume_budget_is_shared_across_failure_forms() {
    #[derive(Clone)]
    struct MixedFailureModel {
        calls: Arc<AtomicUsize>,
        captured: Arc<Mutex<Vec<CapturedProviderInput>>>,
    }

    #[async_trait]
    impl ModelAdapter for MixedFailureModel {
        fn model_name(&self) -> &'static str {
            "stream-mixed-failure"
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
            settings: Option<ModelSettings>,
            params: ModelRequestParameters,
            context: ModelRequestContext,
        ) -> Result<ModelResponseEventStream, ModelError> {
            self.captured.lock().unwrap().push(CapturedProviderInput {
                messages,
                settings,
                params,
                context,
            });
            let call_index = self.calls.fetch_add(1, Ordering::SeqCst);
            if call_index == 0 {
                return Err(ModelError::Transport(
                    "provider disconnected before stream start".to_string(),
                ));
            }
            let (sender, receiver) = tokio::sync::mpsc::channel(1);
            if call_index == 1 {
                sender
                    .send(Err(ModelError::Transport(
                        "provider disconnected while streaming".to_string(),
                    )))
                    .await
                    .unwrap();
            } else if call_index > 2 {
                sender
                    .send(Ok(ModelResponseStreamEvent::FinalResult(Box::new(
                        ModelResponse::text("must not be reached"),
                    ))))
                    .await
                    .unwrap();
            }
            Ok(ModelResponseEventStream::new(receiver))
        }
    }

    let model = Arc::new(MixedFailureModel {
        calls: Arc::new(AtomicUsize::new(0)),
        captured: Arc::new(Mutex::new(Vec::new())),
    });
    let recorder = Arc::new(InMemoryTraceRecorder::new());
    let mut context = AgentContext::default();
    let mut events = Vec::new();

    let error = Agent::new(model.clone())
        .with_trace_recorder(recorder.clone())
        .run_with_context_and_stream_events("continue", &mut context, &mut events)
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        AgentError::Capability(ref message)
            if message == "model stream did not produce a final result"
    ));
    assert_eq!(model.calls.load(Ordering::SeqCst), 3);
    let captured = model.captured.lock().unwrap().clone();
    assert_eq!(captured.len(), 3);
    assert!(captured[1..].iter().all(|input| input == &captured[0]));
    assert_model_attempt_steps(&events, &[0]);
    let resume_retries = context
        .events
        .events()
        .iter()
        .filter(|event| event.kind == "model_stream_resume")
        .map(|event| event.payload["retry"].clone())
        .collect::<Vec<_>>();
    assert_eq!(
        resume_retries,
        vec![serde_json::json!(1), serde_json::json!(2)]
    );
    assert!(!context.runtime.lifecycle.entered);
    assert!(context.runtime.run_toolsets_closed);
    assert!(context.ended_at.is_some());
    assert_eq!(
        context
            .events
            .events()
            .iter()
            .filter(|event| event.kind == "run_failed")
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|record| matches!(record.event, AgentStreamEvent::RunFailed { .. }))
            .count(),
        1
    );
    assert!(matches!(
        events.last().map(|record| &record.event),
        Some(AgentStreamEvent::RunFailed { error_kind, .. })
            if error_kind == "capability_error"
    ));
    let spans = recorder.spans();
    for span_name in [
        "gen_ai.inference",
        "starweaver.loop.step",
        "gen_ai.invoke_agent",
    ] {
        let span = spans.iter().find(|span| span.name == span_name).unwrap();
        assert_eq!(
            span.status,
            SpanStatus::Error {
                error_type: "missing_final_result".to_string(),
            }
        );
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn provider_stream_transport_error_resumes_incremental_request() {
    #[derive(Clone)]
    struct StreamTransportErrorThenResponseModel {
        calls: Arc<AtomicUsize>,
        captured: Arc<Mutex<Vec<CapturedProviderInput>>>,
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
            settings: Option<ModelSettings>,
            params: ModelRequestParameters,
            context: ModelRequestContext,
        ) -> Result<ModelResponseEventStream, ModelError> {
            self.captured.lock().unwrap().push(CapturedProviderInput {
                messages,
                settings,
                params,
                context,
            });
            let call_index = self.calls.fetch_add(1, Ordering::SeqCst);
            let (sender, receiver) = tokio::sync::mpsc::channel(4);
            if call_index == 0 {
                sender
                    .send(Ok(ModelResponseStreamEvent::Diagnostic(
                        StreamDiagnostic::new(
                            "provider_before_resume",
                            serde_json::json!({"attempt": 1}),
                        ),
                    )))
                    .await
                    .unwrap();
                sender
                    .send(Ok(ModelResponseStreamEvent::PartDelta(PartDelta::text(
                        0, "partial",
                    ))))
                    .await
                    .unwrap();
                sender
                    .send(Ok(ModelResponseStreamEvent::FinalResult(Box::new(
                        ModelResponse::text("stale final before disconnect"),
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
                    .send(Ok(ModelResponseStreamEvent::Diagnostic(
                        StreamDiagnostic::new(
                            "provider_after_resume",
                            serde_json::json!({"attempt": 2}),
                        ),
                    )))
                    .await
                    .unwrap();
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
    let recorder = Arc::new(InMemoryTraceRecorder::new());
    let observer = Arc::new(StreamRecordObserver::default());
    let mut context = AgentContext::default();
    let mut events = Vec::new();

    let result = Agent::new(model.clone())
        .with_trace_recorder(recorder.clone())
        .with_stream_observer(observer.clone())
        .run_with_context_and_stream_events("continue", &mut context, &mut events)
        .await
        .unwrap();

    assert_eq!(result.output, "stream resumed");
    assert_eq!(result.state.run_step, 1);
    assert_eq!(model.calls.load(Ordering::SeqCst), 2);
    let captured = model.captured.lock().unwrap().clone();
    assert_eq!(captured.len(), 2);
    assert_eq!(captured[0], captured[1]);
    assert_model_attempt_steps(&events, &[0]);
    assert!(
        context
            .events
            .events()
            .iter()
            .any(|event| event.kind == "model_stream_resume")
    );
    let diagnostic_before_index = events
        .iter()
        .position(|record| {
            matches!(
                &record.event,
                AgentStreamEvent::Custom { event } if event.kind == "provider_before_resume"
            )
        })
        .unwrap();
    let partial_index = events
        .iter()
        .position(|record| {
            matches!(
                &record.event,
                AgentStreamEvent::ModelStream {
                    event: ModelResponseStreamEvent::PartDelta(_),
                    ..
                }
            )
        })
        .unwrap();
    let resume_index = events
        .iter()
        .position(|record| {
            matches!(
                &record.event,
                AgentStreamEvent::Custom { event } if event.kind == "model_stream_resume"
            )
        })
        .unwrap();
    let stale_final_index = events
        .iter()
        .position(|record| {
            matches!(
                &record.event,
                AgentStreamEvent::ModelStream {
                    event: ModelResponseStreamEvent::FinalResult(response),
                    ..
                } if response.text_output() == "stale final before disconnect"
            )
        })
        .unwrap();
    let diagnostic_after_index = events
        .iter()
        .position(|record| {
            matches!(
                &record.event,
                AgentStreamEvent::Custom { event } if event.kind == "provider_after_resume"
            )
        })
        .unwrap();
    let final_index = events
        .iter()
        .rposition(|record| {
            matches!(
                &record.event,
                AgentStreamEvent::ModelStream {
                    event: ModelResponseStreamEvent::FinalResult(response),
                    ..
                } if response.text_output() == "stream resumed"
            )
        })
        .unwrap();
    let response_index = events
        .iter()
        .position(|record| matches!(record.event, AgentStreamEvent::ModelResponse { .. }))
        .unwrap();
    assert!(diagnostic_before_index < partial_index);
    assert!(partial_index < stale_final_index);
    assert!(stale_final_index < resume_index);
    assert!(resume_index < diagnostic_after_index);
    assert!(diagnostic_after_index < final_index);
    assert!(final_index < response_index);
    assert!(!events.iter().any(|record| matches!(
        &record.event,
        AgentStreamEvent::ModelStream {
            event: ModelResponseStreamEvent::Diagnostic(_),
            ..
        }
    )));
    assert_eq!(
        observer.records.lock().unwrap().as_slice(),
        events.as_slice()
    );
    let spans = recorder.spans();
    let model_spans = spans
        .iter()
        .filter(|span| span.name == "gen_ai.inference")
        .collect::<Vec<_>>();
    assert_eq!(model_spans.len(), 1);
    assert_eq!(model_spans[0].status, SpanStatus::Ok);
    assert_eq!(
        model_spans[0]
            .events
            .iter()
            .filter(|event| event.name == "starweaver.model.request")
            .count(),
        1
    );
    assert_eq!(
        model_spans[0]
            .events
            .iter()
            .filter(|event| event.name == "starweaver.model.response")
            .count(),
        1
    );
    assert!(events.iter().any(|record| matches!(
        &record.event,
        starweaver_runtime::AgentStreamEvent::Custom { event }
            if event.kind == "model_stream_resume"
    )));
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn provider_stream_clean_close_without_final_resumes_incremental_request() {
    #[derive(Clone)]
    struct StreamCloseThenResponseModel {
        calls: Arc<AtomicUsize>,
        captured: Arc<Mutex<Vec<CapturedProviderInput>>>,
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
            settings: Option<ModelSettings>,
            params: ModelRequestParameters,
            context: ModelRequestContext,
        ) -> Result<ModelResponseEventStream, ModelError> {
            self.captured.lock().unwrap().push(CapturedProviderInput {
                messages,
                settings,
                params,
                context,
            });
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
    assert_eq!(result.state.run_step, 1);
    assert_eq!(model.calls.load(Ordering::SeqCst), 2);
    let captured = model.captured.lock().unwrap().clone();
    assert_eq!(captured.len(), 2);
    assert_eq!(captured[0], captured[1]);
    assert_model_attempt_steps(&events, &[0]);
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
#[allow(clippy::too_many_lines)]
async fn provider_stream_cancellation_does_not_resume() {
    #[derive(Clone)]
    struct CancelledStreamModel {
        calls: Arc<AtomicUsize>,
        cancel_during_stream: bool,
    }

    #[async_trait]
    impl ModelAdapter for CancelledStreamModel {
        fn model_name(&self) -> &'static str {
            "cancelled-stream"
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
            _messages: Vec<ModelMessage>,
            _settings: Option<ModelSettings>,
            _params: ModelRequestParameters,
            _context: ModelRequestContext,
        ) -> Result<ModelResponseEventStream, ModelError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let cancellation = ModelError::Cancelled {
                reason: "cancelled by test".to_string(),
            };
            if !self.cancel_during_stream {
                return Err(cancellation);
            }
            let (sender, receiver) = tokio::sync::mpsc::channel(1);
            sender.send(Err(cancellation)).await.unwrap();
            Ok(ModelResponseEventStream::new(receiver))
        }
    }

    for cancel_during_stream in [false, true] {
        let model = Arc::new(CancelledStreamModel {
            calls: Arc::new(AtomicUsize::new(0)),
            cancel_during_stream,
        });
        let mut context = AgentContext::default();
        let mut events = Vec::new();

        let error = Agent::new(model.clone())
            .run_with_context_and_stream_events("continue", &mut context, &mut events)
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            AgentError::Cancelled { ref reason } if reason == "cancelled by test"
        ));
        assert_eq!(model.calls.load(Ordering::SeqCst), 1);
        assert_model_attempt_steps(&events, &[0]);
        assert!(
            !context
                .events
                .events()
                .iter()
                .any(|event| event.kind == "model_stream_resume")
        );
        assert!(!events.iter().any(|record| matches!(
            &record.event,
            AgentStreamEvent::Custom { event } if event.kind == "model_stream_resume"
        )));
    }
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
