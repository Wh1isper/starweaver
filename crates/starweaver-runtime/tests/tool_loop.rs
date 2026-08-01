#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::{
    Arc, LazyLock, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use starweaver_context::AgentContext;
use starweaver_core::Metadata;
use starweaver_model::{
    ModelAdapter, ModelError, ModelMessage, ModelProfile, ModelRequestContext,
    ModelRequestParameters, ModelRequestPart, ModelResponse, ModelResponseEventStream,
    ModelResponsePart, ModelRunSession, ModelSettings, ProtocolFamily, ToolCallPart,
    ToolDefinition, ToolReturnPart,
};
use starweaver_runtime::{
    Agent, AgentCapability, AgentRunState, AgentRuntimePolicy, AgentStreamEvent, AgentStreamRecord,
    AgentToolExecutionMode, CapabilityResult,
};
use starweaver_tools::{
    CodeActEligibility, DynTool, FunctionTool, NestedToolInvoker, TOOL_METADATA_DEPENDENCIES_KEY,
    TOOL_METADATA_NESTED_INVOKER_KEY, ToolContext, ToolDependencyRequirements, ToolError,
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

fn strict_metadata() -> Metadata {
    Metadata::from_iter([(
        TOOL_METADATA_DEPENDENCIES_KEY.to_string(),
        ToolDependencyRequirements::strict(Vec::<String>::new(), Vec::<String>::new(), false)
            .to_metadata_value(),
    )])
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

fn constrained_orchestrator_registry(
    target: DynTool,
    outer_timeout_ms: Option<u64>,
) -> ToolRegistry {
    constrained_registry_with_outer_name(target, outer_timeout_ms, "orchestrate")
}

fn constrained_registry_with_outer_name(
    target: DynTool,
    outer_timeout_ms: Option<u64>,
    outer_name: &'static str,
) -> ToolRegistry {
    let mut outer_metadata = strict_metadata();
    outer_metadata.insert(
        TOOL_METADATA_NESTED_INVOKER_KEY.to_string(),
        serde_json::json!(true),
    );
    let mut orchestrator = FunctionTool::new(
        outer_name,
        Some("synthetic constrained orchestrator".to_string()),
        serde_json::json!({"type": "object"}),
        |context: ToolContext, _arguments: serde_json::Value| async move {
            let invoker =
                context
                    .dependency::<NestedToolInvoker>()
                    .ok_or_else(|| ToolError::UserError {
                        tool: "orchestrate".to_string(),
                        message: "missing nested invoker".to_string(),
                    })?;
            let result = invoker
                .invoke("nested_target", serde_json::json!({"value": 7}))
                .await
                .map_err(|error| ToolError::UserError {
                    tool: "orchestrate".to_string(),
                    message: error.to_string(),
                })?;
            Ok(ToolResult::new(result.content))
        },
    )
    .with_metadata(outer_metadata)
    .with_max_retries(0)
    .with_sequential(true)
    .with_codeact(false);
    if let Some(timeout_ms) = outer_timeout_ms {
        orchestrator = orchestrator.with_timeout_ms(timeout_ms);
    }
    ToolRegistry::new()
        .with_tool(target)
        .with_tool(Arc::new(orchestrator))
}

fn orchestrator_model() -> Arc<ScriptedModel> {
    Arc::new(ScriptedModel::new(vec![
        ModelResponse {
            parts: vec![ModelResponsePart::ToolCall(ToolCallPart {
                id: "call_orchestrate".to_string(),
                name: "orchestrate".to_string(),
                arguments: serde_json::json!({}).into(),
            })],
            ..ModelResponse::text("")
        },
        ModelResponse::text("done"),
    ]))
}

#[tokio::test]
async fn constrained_orchestrator_reenters_canonical_tool_pipeline_without_child_history() {
    let model = Arc::new(ScriptedModel::new(vec![
        ModelResponse {
            parts: vec![ModelResponsePart::ToolCall(ToolCallPart {
                id: "call_orchestrate".to_string(),
                name: "orchestrate".to_string(),
                arguments: serde_json::json!({}).into(),
            })],
            ..ModelResponse::text("")
        },
        ModelResponse::text("done"),
    ]));
    let effects = Arc::new(AtomicUsize::new(0));
    let target_effects = Arc::clone(&effects);
    let target = FunctionTool::new(
        "nested_target",
        Some("nested target".to_string()),
        serde_json::json!({"type": "object"}),
        move |_context: ToolContext, arguments: serde_json::Value| {
            let target_effects = Arc::clone(&target_effects);
            async move {
                target_effects.fetch_add(1, Ordering::SeqCst);
                Ok(ToolResult::new(serde_json::json!({
                    "nested": true,
                    "arguments": arguments,
                })))
            }
        },
    )
    .with_metadata(strict_metadata());
    let result = Agent::new(model.clone())
        .with_tools(constrained_orchestrator_registry(Arc::new(target), None))
        .run("compose")
        .await
        .unwrap();

    assert_eq!(result.output, "done");
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    let second_request = format!("{:?}", model.captured.lock().unwrap()[1]);
    assert!(second_request.contains("orchestrate"));
    assert!(!second_request.contains("nested_target"));
}

#[tokio::test]
async fn oversized_codeact_catalog_fails_closed_to_an_empty_nested_allowlist() {
    let model = Arc::new(ScriptedModel::new(vec![ModelResponse::text("done")]));
    let target = FunctionTool::new(
        "nested_target",
        Some("x".repeat(40 * 1024)),
        serde_json::json!({"type": "object"}),
        |_context: ToolContext, arguments: serde_json::Value| async move {
            Ok(ToolResult::new(arguments))
        },
    )
    .with_metadata(strict_metadata());
    let mut context = AgentContext::default();

    let result = Agent::new(model.clone())
        .with_tools(constrained_registry_with_outer_name(
            Arc::new(target),
            None,
            "run_code",
        ))
        .run_with_context("compose", &mut context)
        .await
        .unwrap();

    assert_eq!(result.output, "done");
    assert!(context.runtime.prepared_codeact_target_names.is_empty());
    let params = model.captured_params.lock().unwrap()[0].clone();
    let catalog = params
        .instructions
        .iter()
        .find(|instruction| instruction.text.contains("<codeact_catalog>"))
        .unwrap();
    assert!(catalog.text.contains(": []</codeact_catalog>"));
}

#[derive(Clone, Copy)]
enum HangingNestedHookPhase {
    Before,
    After,
}

struct HangingNestedHook {
    phase: HangingNestedHookPhase,
}

#[async_trait]
impl AgentCapability for HangingNestedHook {
    async fn before_tool_execution(
        &self,
        _state: &mut AgentRunState,
        _tool_context: &mut ToolContext,
        call: &ToolCallPart,
    ) -> CapabilityResult<()> {
        if call.name == "nested_target" && matches!(self.phase, HangingNestedHookPhase::Before) {
            std::future::pending().await
        } else {
            Ok(())
        }
    }

    async fn after_tool_result(
        &self,
        _state: &mut AgentRunState,
        call: &ToolCallPart,
        _tool_return: &mut ToolReturnPart,
    ) -> CapabilityResult<()> {
        if call.name == "nested_target" && matches!(self.phase, HangingNestedHookPhase::After) {
            std::future::pending().await
        } else {
            Ok(())
        }
    }
}

struct HangingNestedStreamObserver;

#[async_trait]
impl AgentCapability for HangingNestedStreamObserver {
    async fn on_stream_event(
        &self,
        _state: &AgentRunState,
        record: &AgentStreamRecord,
    ) -> CapabilityResult<()> {
        if matches!(
            &record.event,
            AgentStreamEvent::ToolCall { call, .. } if call.name == "nested_target"
        ) {
            std::future::pending().await
        } else {
            Ok(())
        }
    }
}

fn immediate_nested_target() -> DynTool {
    Arc::new(
        FunctionTool::new(
            "nested_target",
            Some("nested target".to_string()),
            serde_json::json!({"type": "object"}),
            |_context: ToolContext, arguments: serde_json::Value| async move {
                Ok(ToolResult::new(arguments))
            },
        )
        .with_metadata(strict_metadata()),
    )
}

#[tokio::test]
async fn outer_tool_timeout_interrupts_hanging_nested_before_hook() {
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        Agent::new(orchestrator_model())
            .with_tools(constrained_orchestrator_registry(
                immediate_nested_target(),
                Some(20),
            ))
            .with_capability(Arc::new(HangingNestedHook {
                phase: HangingNestedHookPhase::Before,
            }))
            .run("compose"),
    )
    .await
    .unwrap_or_else(|error| panic!("outer timeout was blocked by the nested before hook: {error}"))
    .unwrap();

    assert_eq!(result.output, "done");
}

#[tokio::test]
async fn outer_tool_timeout_interrupts_hanging_nested_after_hook() {
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        Agent::new(orchestrator_model())
            .with_tools(constrained_orchestrator_registry(
                immediate_nested_target(),
                Some(20),
            ))
            .with_capability(Arc::new(HangingNestedHook {
                phase: HangingNestedHookPhase::After,
            }))
            .run("compose"),
    )
    .await
    .unwrap_or_else(|error| panic!("outer timeout was blocked by the nested after hook: {error}"))
    .unwrap();

    assert_eq!(result.output, "done");
}

#[tokio::test]
async fn outer_tool_timeout_interrupts_hanging_nested_stream_observer() {
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        Agent::new(orchestrator_model())
            .with_tools(constrained_orchestrator_registry(
                immediate_nested_target(),
                Some(20),
            ))
            .with_stream_observer(Arc::new(HangingNestedStreamObserver))
            .run_stream("compose"),
    )
    .await
    .unwrap_or_else(|error| {
        panic!("outer timeout was blocked by the nested stream observer: {error}")
    })
    .unwrap();

    assert_eq!(result.result.output, "done");
}

struct PreparedSchemaCapability;

#[async_trait]
impl AgentCapability for PreparedSchemaCapability {
    async fn prepare_tools(
        &self,
        _state: &AgentRunState,
        mut tools: Vec<ToolDefinition>,
    ) -> CapabilityResult<Vec<ToolDefinition>> {
        if let Some(target) = tools.iter_mut().find(|tool| tool.name == "nested_target") {
            target.description = Some("prepared nested target".to_string());
            target.parameters = serde_json::json!({
                "type": "object",
                "properties": {"value": {"type": "integer", "const": 7}},
                "required": ["value"],
                "additionalProperties": false
            });
        }
        Ok(tools)
    }
}

struct EligibilityFlippingModel {
    eligible: Arc<std::sync::atomic::AtomicBool>,
    requests: AtomicUsize,
    captured: Arc<Mutex<Vec<ModelRequestParameters>>>,
}

#[async_trait]
impl ModelAdapter for EligibilityFlippingModel {
    fn model_name(&self) -> &'static str {
        "eligibility-flipping"
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
        params: ModelRequestParameters,
        _context: ModelRequestContext,
    ) -> Result<ModelResponse, ModelError> {
        self.captured.lock().unwrap().push(params);
        if self.requests.fetch_add(1, Ordering::SeqCst) == 0 {
            self.eligible.store(false, Ordering::SeqCst);
            Ok(ModelResponse {
                parts: vec![ModelResponsePart::ToolCall(ToolCallPart {
                    id: "call_run_code".to_string(),
                    name: "run_code".to_string(),
                    arguments: serde_json::json!({}).into(),
                })],
                ..ModelResponse::text("")
            })
        } else {
            Ok(ModelResponse::text("done"))
        }
    }
}

#[tokio::test]
async fn nested_admission_and_catalog_use_the_frozen_prepared_request_snapshot() {
    let eligible = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let captured = Arc::new(Mutex::new(Vec::new()));
    let model = Arc::new(EligibilityFlippingModel {
        eligible: Arc::clone(&eligible),
        requests: AtomicUsize::new(0),
        captured: Arc::clone(&captured),
    });
    let effects = Arc::new(AtomicUsize::new(0));
    let target_effects = Arc::clone(&effects);
    let eligibility = Arc::clone(&eligible);
    let target = FunctionTool::new(
        "nested_target",
        Some("unprepared nested target".to_string()),
        serde_json::json!({"type": "object"}),
        move |_context: ToolContext, arguments: serde_json::Value| {
            let target_effects = Arc::clone(&target_effects);
            async move {
                target_effects.fetch_add(1, Ordering::SeqCst);
                Ok(ToolResult::new(arguments))
            }
        },
    )
    .with_metadata(strict_metadata())
    .with_codeact_availability(move |_| {
        if eligibility.load(Ordering::SeqCst) {
            CodeActEligibility::Allow
        } else {
            CodeActEligibility::Deny
        }
    });

    let result = Agent::new(model)
        .with_tools(constrained_registry_with_outer_name(
            Arc::new(target),
            None,
            "run_code",
        ))
        .with_capability(Arc::new(PreparedSchemaCapability))
        .run("compose")
        .await
        .unwrap();

    assert_eq!(result.output, "done");
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    let first_params = captured.lock().unwrap()[0].clone();
    let prepared_target = first_params
        .tools
        .iter()
        .find(|tool| tool.name == "nested_target")
        .unwrap();
    assert_eq!(
        prepared_target.description.as_deref(),
        Some("prepared nested target")
    );
    assert_eq!(
        prepared_target.parameters["properties"]["value"]["const"],
        7
    );
    let catalog = first_params
        .instructions
        .iter()
        .find(|instruction| instruction.text.contains("<codeact_catalog>"))
        .unwrap();
    assert!(catalog.text.contains("prepared nested target"));
    assert!(catalog.text.contains("\"const\":7"));
}
