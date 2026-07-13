#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::{
    Arc, LazyLock, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use starweaver_core::Metadata;
use starweaver_model::{
    ModelAdapter, ModelError, ModelMessage, ModelProfile, ModelRequestContext,
    ModelRequestParameters, ModelRequestPart, ModelResponse, ModelResponseEventStream,
    ModelResponsePart, ModelRunSession, ModelSettings, ProtocolFamily, ToolCallPart,
};
use starweaver_runtime::{Agent, AgentRuntimePolicy, AgentToolExecutionMode};
use starweaver_tools::{
    FunctionTool, TOOL_METADATA_DEPENDENCIES_KEY, ToolContext, ToolDependencyRequirements,
    ToolRegistry, ToolResult,
};

#[derive(Clone)]
struct ScriptedModel {
    responses: Arc<Mutex<Vec<ModelResponse>>>,
    captured: Arc<Mutex<Vec<Vec<ModelMessage>>>>,
    captured_settings: Arc<Mutex<Vec<Option<ModelSettings>>>>,
    captured_params: Arc<Mutex<Vec<ModelRequestParameters>>>,
    defaults: Option<ModelSettings>,
}

struct SessionCountingModel {
    responses: Arc<Mutex<Vec<ModelResponse>>>,
    sessions_started: Arc<AtomicUsize>,
    session_requests: Arc<AtomicUsize>,
}

struct SessionCountingRunSession<'a> {
    model: &'a SessionCountingModel,
}

impl SessionCountingModel {
    fn new(responses: Vec<ModelResponse>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses.into_iter().rev().collect())),
            sessions_started: Arc::new(AtomicUsize::new(0)),
            session_requests: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl ScriptedModel {
    fn new(responses: Vec<ModelResponse>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses.into_iter().rev().collect())),
            captured: Arc::new(Mutex::new(Vec::new())),
            captured_settings: Arc::new(Mutex::new(Vec::new())),
            captured_params: Arc::new(Mutex::new(Vec::new())),
            defaults: None,
        }
    }

    fn with_defaults(mut self, defaults: ModelSettings) -> Self {
        self.defaults = Some(defaults);
        self
    }
}

#[async_trait]
impl ModelAdapter for ScriptedModel {
    fn model_name(&self) -> &'static str {
        "scripted"
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
        self.defaults.as_ref()
    }

    async fn request(
        &self,
        messages: Vec<ModelMessage>,
        settings: Option<ModelSettings>,
        params: ModelRequestParameters,
        _context: ModelRequestContext,
    ) -> Result<ModelResponse, ModelError> {
        self.captured.lock().unwrap().push(messages);
        self.captured_settings.lock().unwrap().push(settings);
        self.captured_params.lock().unwrap().push(params);
        self.responses
            .lock()
            .unwrap()
            .pop()
            .ok_or_else(|| ModelError::Transport("script exhausted".to_string()))
    }
}

#[async_trait]
impl ModelAdapter for SessionCountingModel {
    fn model_name(&self) -> &'static str {
        "session-counting"
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

    fn start_run_session(&self) -> Box<dyn ModelRunSession + '_> {
        self.sessions_started.fetch_add(1, Ordering::SeqCst);
        Box::new(SessionCountingRunSession { model: self })
    }

    async fn request(
        &self,
        _messages: Vec<ModelMessage>,
        _settings: Option<ModelSettings>,
        _params: ModelRequestParameters,
        _context: ModelRequestContext,
    ) -> Result<ModelResponse, ModelError> {
        Err(ModelError::Transport(
            "session-counting model must be called through a run session".to_string(),
        ))
    }
}

#[async_trait]
impl ModelRunSession for SessionCountingRunSession<'_> {
    async fn request_stream_incremental(
        &mut self,
        _messages: Vec<ModelMessage>,
        _settings: Option<ModelSettings>,
        _params: ModelRequestParameters,
        context: ModelRequestContext,
    ) -> Result<ModelResponseEventStream, ModelError> {
        let response = self
            .request_stream_final(
                Vec::new(),
                None,
                ModelRequestParameters::default(),
                context.clone(),
            )
            .await?;
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        let _ = sender
            .send(Ok(starweaver_model::ModelResponseStreamEvent::FinalResult(
                Box::new(response),
            )))
            .await;
        Ok(ModelResponseEventStream::new_with_cancellation(
            receiver,
            context.cancellation_token(),
        ))
    }

    async fn request_stream_final(
        &mut self,
        _messages: Vec<ModelMessage>,
        _settings: Option<ModelSettings>,
        _params: ModelRequestParameters,
        _context: ModelRequestContext,
    ) -> Result<ModelResponse, ModelError> {
        self.model.session_requests.fetch_add(1, Ordering::SeqCst);
        self.model
            .responses
            .lock()
            .unwrap()
            .pop()
            .ok_or_else(|| ModelError::Transport("script exhausted".to_string()))
    }
}

fn lookup_registry() -> ToolRegistry {
    let tool = FunctionTool::new(
        "lookup",
        Some("Lookup a value".to_string()),
        serde_json::json!({
            "type": "object",
            "properties": {"query": {"type": "string"}},
            "required": ["query"]
        }),
        |_ctx: ToolContext, args: serde_json::Value| async move {
            Ok(ToolResult::new(serde_json::json!({
                "value": args["query"].as_str().unwrap_or_default()
            })))
        },
    );
    ToolRegistry::new().with_tool(Arc::new(tool))
}

fn request_tool_return_names(messages: &[ModelMessage]) -> Vec<String> {
    let Some(ModelMessage::Request(request)) = messages.last() else {
        return Vec::new();
    };
    request
        .parts
        .iter()
        .filter_map(|part| match part {
            ModelRequestPart::ToolReturn(tool_return) => Some(tool_return.name.clone()),
            _ => None,
        })
        .collect()
}

fn record_tool_start(current: &AtomicUsize, max_seen: &AtomicUsize) {
    let active = current.fetch_add(1, Ordering::SeqCst) + 1;
    max_seen.fetch_max(active, Ordering::SeqCst);
}

fn record_tool_finish(current: &AtomicUsize) {
    current.fetch_sub(1, Ordering::SeqCst);
}

#[tokio::test]
async fn agent_executes_tool_calls_and_continues_model_loop() {
    let model = Arc::new(ScriptedModel::new(vec![
        ModelResponse {
            parts: vec![ModelResponsePart::ToolCall(ToolCallPart {
                id: "call_1".to_string(),
                name: "lookup".to_string(),
                arguments: serde_json::json!({"query": "Paris"}).into(),
            })],
            ..ModelResponse::text("")
        },
        ModelResponse::text("Paris result"),
    ]));

    let result = Agent::new(model.clone())
        .with_tools(lookup_registry())
        .run("lookup Paris")
        .await
        .unwrap();

    assert_eq!(result.output, "Paris result");
    assert_eq!(result.messages.len(), 4);
    assert_eq!(result.new_messages().len(), 4);
    let second_request_history = model.captured.lock().unwrap()[1].clone();
    let second_request = second_request_history.last().unwrap();
    assert!(format!("{second_request:?}").contains("ToolReturn"));
    assert!(format!("{second_request:?}").contains("Paris"));
}

#[tokio::test]
async fn agent_executes_distinct_filtered_tool_calls_in_parallel_and_preserves_order() {
    let model = Arc::new(ScriptedModel::new(vec![
        ModelResponse {
            parts: vec![
                ModelResponsePart::ToolCall(ToolCallPart {
                    id: "call_alpha".to_string(),
                    name: "alpha".to_string(),
                    arguments: serde_json::json!({}).into(),
                }),
                ModelResponsePart::ToolCall(ToolCallPart {
                    id: "call_beta".to_string(),
                    name: "beta".to_string(),
                    arguments: serde_json::json!({}).into(),
                }),
            ],
            ..ModelResponse::text("")
        },
        ModelResponse::text("done"),
    ]));
    let current = Arc::new(AtomicUsize::new(0));
    let max_seen = Arc::new(AtomicUsize::new(0));
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let metadata = Metadata::from_iter([(
        TOOL_METADATA_DEPENDENCIES_KEY.to_string(),
        ToolDependencyRequirements::filtered(std::iter::empty::<String>(), false)
            .to_metadata_value(),
    )]);

    let alpha = {
        let current = Arc::clone(&current);
        let max_seen = Arc::clone(&max_seen);
        let barrier = Arc::clone(&barrier);
        FunctionTool::new(
            "alpha",
            Some("Alpha".to_string()),
            serde_json::json!({"type": "object"}),
            move |_ctx: ToolContext, _args| {
                let current = Arc::clone(&current);
                let max_seen = Arc::clone(&max_seen);
                let barrier = Arc::clone(&barrier);
                async move {
                    record_tool_start(&current, &max_seen);
                    barrier.wait().await;
                    tokio::time::sleep(std::time::Duration::from_millis(60)).await;
                    record_tool_finish(&current);
                    Ok(ToolResult::new(serde_json::json!({"tool": "alpha"})))
                }
            },
        )
        .with_metadata(metadata.clone())
    };
    let beta = {
        let current = Arc::clone(&current);
        let max_seen = Arc::clone(&max_seen);
        let barrier = Arc::clone(&barrier);
        FunctionTool::new(
            "beta",
            Some("Beta".to_string()),
            serde_json::json!({"type": "object"}),
            move |_ctx: ToolContext, _args| {
                let current = Arc::clone(&current);
                let max_seen = Arc::clone(&max_seen);
                let barrier = Arc::clone(&barrier);
                async move {
                    record_tool_start(&current, &max_seen);
                    barrier.wait().await;
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    record_tool_finish(&current);
                    Ok(ToolResult::new(serde_json::json!({"tool": "beta"})))
                }
            },
        )
        .with_metadata(metadata)
    };

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        Agent::new(model.clone())
            .with_tools(
                ToolRegistry::new()
                    .with_tool(Arc::new(alpha))
                    .with_tool(Arc::new(beta)),
            )
            .run("run tools"),
    )
    .await
    .unwrap()
    .unwrap();

    assert_eq!(result.output, "done");
    assert_eq!(max_seen.load(Ordering::SeqCst), 2);
    let return_names = {
        let captured = model.captured.lock().unwrap();
        request_tool_return_names(&captured[1])
    };
    assert_eq!(return_names, vec!["alpha".to_string(), "beta".to_string()]);
}

#[tokio::test]
async fn agent_respects_sequential_tool_execution_policy() {
    let model = Arc::new(ScriptedModel::new(vec![
        ModelResponse {
            parts: vec![
                ModelResponsePart::ToolCall(ToolCallPart {
                    id: "call_alpha".to_string(),
                    name: "alpha".to_string(),
                    arguments: serde_json::json!({}).into(),
                }),
                ModelResponsePart::ToolCall(ToolCallPart {
                    id: "call_beta".to_string(),
                    name: "beta".to_string(),
                    arguments: serde_json::json!({}).into(),
                }),
            ],
            ..ModelResponse::text("")
        },
        ModelResponse::text("done"),
    ]));
    let current = Arc::new(AtomicUsize::new(0));
    let max_seen = Arc::new(AtomicUsize::new(0));

    let tool = |name: &'static str| {
        let current = Arc::clone(&current);
        let max_seen = Arc::clone(&max_seen);
        FunctionTool::new(
            name,
            Some(format!("{name} tool")),
            serde_json::json!({"type": "object"}),
            move |_ctx: ToolContext, _args| {
                let current = Arc::clone(&current);
                let max_seen = Arc::clone(&max_seen);
                async move {
                    record_tool_start(&current, &max_seen);
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    record_tool_finish(&current);
                    Ok(ToolResult::new(serde_json::json!({"tool": name})))
                }
            },
        )
    };

    let result = Agent::new(model)
        .with_tools(
            ToolRegistry::new()
                .with_tool(Arc::new(tool("alpha")))
                .with_tool(Arc::new(tool("beta"))),
        )
        .with_policy(AgentRuntimePolicy {
            tool_execution: AgentToolExecutionMode::Sequential,
            ..AgentRuntimePolicy::default()
        })
        .run("run tools")
        .await
        .unwrap();

    assert_eq!(result.output, "done");
    assert_eq!(max_seen.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn agent_loop_reuses_one_model_run_session_across_tool_continuation() {
    let model = Arc::new(SessionCountingModel::new(vec![
        ModelResponse {
            parts: vec![ModelResponsePart::ToolCall(ToolCallPart {
                id: "call_1".to_string(),
                name: "lookup".to_string(),
                arguments: serde_json::json!({"query": "Paris"}).into(),
            })],
            ..ModelResponse::text("")
        },
        ModelResponse::text("Paris result"),
    ]));

    let result = Agent::new(model.clone())
        .with_tools(lookup_registry())
        .run("lookup Paris")
        .await
        .unwrap();

    assert_eq!(result.output, "Paris result");
    assert_eq!(model.sessions_started.load(Ordering::SeqCst), 1);
    assert_eq!(model.session_requests.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn agent_continues_with_prior_message_history() {
    let first_model = Arc::new(ScriptedModel::new(vec![ModelResponse::text("first")]));
    let first = Agent::new(first_model).run("first prompt").await.unwrap();

    let second_model = Arc::new(ScriptedModel::new(vec![ModelResponse::text("second")]));
    let second = Agent::new(second_model.clone())
        .run_with_history("second prompt", first.new_messages().to_vec())
        .await
        .unwrap();

    assert_eq!(second.output, "second");
    assert_eq!(second.history_len, 2);
    assert_eq!(second.new_messages().len(), 2);
    assert_eq!(second.all_messages().len(), 4);
    let captured = second_model.captured.lock().unwrap()[0].clone();
    assert_eq!(captured.len(), 3);
}

#[tokio::test]
async fn agent_merges_model_default_settings_with_agent_settings() {
    let defaults = ModelSettings {
        max_tokens: Some(128),
        temperature: Some(0.1),
        ..ModelSettings::default()
    };
    let model =
        Arc::new(ScriptedModel::new(vec![ModelResponse::text("ok")]).with_defaults(defaults));

    Agent::new(model.clone())
        .with_model_settings(ModelSettings {
            temperature: Some(0.7),
            ..ModelSettings::default()
        })
        .run("settings")
        .await
        .unwrap();

    let settings = model.captured_settings.lock().unwrap()[0].clone().unwrap();
    assert_eq!(settings.max_tokens, Some(128));
    assert_eq!(settings.temperature, Some(0.7));
}

#[tokio::test]
async fn agent_passes_registered_tool_definitions_to_model() {
    let model = Arc::new(ScriptedModel::new(vec![ModelResponse::text("ok")]));

    Agent::new(model.clone())
        .with_tools(lookup_registry())
        .run("what tools exist")
        .await
        .unwrap();

    let params = model.captured_params.lock().unwrap()[0].clone();
    assert_eq!(params.tools.len(), 1);
    assert_eq!(params.tools[0].name, "lookup");
}
