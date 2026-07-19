#![allow(
    missing_docs,
    clippy::expect_used,
    clippy::significant_drop_tightening,
    clippy::unwrap_used
)]

use std::sync::{Arc, LazyLock, Mutex};

use async_trait::async_trait;
use starweaver_agent::{
    AgentBuilder, AgentCapability, AgentContext, AgentRunState, AgentSession, CapabilityError,
    CapabilityOrdering, CapabilityResult, CapabilitySpec, DEFAULT_FILTER_ORDER, DynToolset,
    FunctionDynamicInstruction, FunctionModel, FunctionModelInfo, FunctionOutputFunction,
    FunctionOutputValidator, FunctionTool, MediaUploadRequest, MediaUploader, ModelCapability,
    ModelConfig, OutputFunctionDefinition, OutputSchema, OutputValue, PerThousandRatio,
    StaticCapabilityBundle, StaticToolset, TOOLSET_CLOSED_EVENT_KIND,
    TOOLSET_INITIALIZED_EVENT_KIND, TestModel, ToolContext, ToolExecutionHook, ToolInstruction,
    ToolRegistry, ToolResult, Toolset, ToolsetLifecycleError, ToolsetLifecyclePolicy,
    ToolsetPreparation, Usage, UsageLimits, attach_environment, context_tools,
};
use starweaver_environment::{
    EnvironmentPolicy, FilePolicy, ShellPolicy, VirtualEnvironmentProvider,
};
use starweaver_model::{
    CONTEXT_ORIGIN_ENVIRONMENT_CONTEXT, CONTEXT_ORIGIN_METADATA, CONTEXT_ORIGIN_RUNTIME_CONTEXT,
    ContentPart, ModelAdapter, ModelError, ModelMessage, ModelProfile, ModelRequest,
    ModelRequestContext, ModelRequestParameters, ModelRequestPart, ModelResponse,
    ModelResponsePart, ModelSettings, ProtocolFamily, ToolCallPart,
    providers::openai_responses::OpenAiResponsesAdapter, tool_call_response,
};

#[derive(Clone)]
struct CaptureModel {
    captured_params: Arc<Mutex<Vec<ModelRequestParameters>>>,
}

struct CustomFilterCapability;
struct FailingRunCompleteCapability;

#[derive(Clone)]
struct MessageCaptureModel {
    captured_messages: Arc<Mutex<Vec<Vec<ModelMessage>>>>,
}

struct ModelConfigCaptureCapability {
    captured_configs: Arc<Mutex<Vec<ModelConfig>>>,
}

struct InlineImageCapability;

struct FakeUploader;

struct FailingUploader;

struct TenantPreparedToolset;

struct RunLifecycleToolset {
    calls: Arc<Mutex<Vec<String>>>,
}

struct BuilderToolHook {
    order: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl ToolExecutionHook for BuilderToolHook {
    async fn before_tool_call(
        &self,
        _context: &mut ToolContext,
        _call: &ToolCallPart,
        arguments: &mut serde_json::Value,
    ) -> Result<(), starweaver_agent::ToolError> {
        self.order.lock().unwrap().push("before".to_string());
        arguments["builder_hook"] = serde_json::json!(true);
        Ok(())
    }
}

#[async_trait]
impl Toolset for RunLifecycleToolset {
    fn name(&self) -> &'static str {
        "run_lifecycle_tools"
    }

    fn get_tools(&self) -> Vec<starweaver_agent::DynTool> {
        Vec::new()
    }

    fn lifecycle_policy(&self) -> ToolsetLifecyclePolicy {
        ToolsetLifecyclePolicy::default()
            .with_enter_before_prepare(true)
            .with_exit_after_run(true)
    }

    async fn enter_with_context(
        &self,
        _context: &AgentContext,
    ) -> Result<starweaver_agent::ToolsetLifecycleReport, ToolsetLifecycleError> {
        self.calls.lock().unwrap().push("enter".to_string());
        Ok(starweaver_agent::ToolsetLifecycleReport::new(
            self.name(),
            self.id().map(ToOwned::to_owned),
            starweaver_agent::ToolsetLifecycleState::Initialized,
            0,
            0,
        ))
    }

    async fn prepare_with_context(
        &self,
        _context: &AgentContext,
    ) -> Result<ToolsetPreparation, ToolsetLifecycleError> {
        self.calls.lock().unwrap().push("prepare".to_string());
        Ok(ToolsetPreparation::initialized(
            self.name(),
            self.id().map(ToOwned::to_owned),
            Vec::new(),
            Vec::new(),
        ))
    }

    async fn exit_with_context(
        &self,
        _context: &AgentContext,
    ) -> Result<starweaver_agent::ToolsetLifecycleReport, ToolsetLifecycleError> {
        self.calls.lock().unwrap().push("exit".to_string());
        Ok(starweaver_agent::ToolsetLifecycleReport::new(
            self.name(),
            self.id().map(ToOwned::to_owned),
            starweaver_agent::ToolsetLifecycleState::Closed,
            0,
            0,
        ))
    }
}

#[async_trait]
impl Toolset for TenantPreparedToolset {
    fn name(&self) -> &'static str {
        "tenant_tools"
    }

    fn get_tools(&self) -> Vec<starweaver_agent::DynTool> {
        Vec::new()
    }

    async fn prepare_with_context(
        &self,
        context: &AgentContext,
    ) -> Result<ToolsetPreparation, ToolsetLifecycleError> {
        assert_eq!(context.metadata["tenant"], "alpha");
        let tool = FunctionTool::new(
            "tenant_echo",
            Some("Tenant echo tool".to_string()),
            serde_json::json!({"type": "object"}),
            |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
        );
        Ok(ToolsetPreparation::initialized(
            self.name(),
            self.id().map(ToOwned::to_owned),
            vec![Arc::new(tool)],
            vec![ToolInstruction::new("tenant_tools", "Use tenant tools.")],
        ))
    }
}

#[async_trait]
impl MediaUploader for FakeUploader {
    async fn upload(&self, request: MediaUploadRequest) -> Result<ContentPart, String> {
        Ok(ContentPart::ResourceRef {
            uri: "resource://builder-upload/image".to_string(),
            media_type: request.media_type,
            resource_type: "image".to_string(),
            metadata: serde_json::Map::new(),
        })
    }
}

#[async_trait]
impl MediaUploader for FailingUploader {
    async fn upload(&self, _request: MediaUploadRequest) -> Result<ContentPart, String> {
        Err("upload unavailable".to_string())
    }
}

#[async_trait]
impl AgentCapability for ModelConfigCaptureCapability {
    fn spec(&self) -> CapabilitySpec {
        CapabilitySpec::new("test.model_config_capture")
            .with_ordering(CapabilityOrdering::default().after("starweaver.filter.capability"))
    }

    async fn prepare_model_messages_with_context(
        &self,
        _state: &mut AgentRunState,
        context: &mut AgentContext,
        messages: Vec<ModelMessage>,
    ) -> CapabilityResult<Vec<ModelMessage>> {
        self.captured_configs
            .lock()
            .unwrap()
            .push(context.model_config.clone());
        Ok(messages)
    }
}

#[async_trait]
impl AgentCapability for InlineImageCapability {
    fn spec(&self) -> CapabilitySpec {
        CapabilitySpec::new("test.inline_image_capability").with_ordering(
            CapabilityOrdering::default().before("starweaver.filter.reasoning_normalize"),
        )
    }

    async fn prepare_model_messages_with_context(
        &self,
        _state: &mut AgentRunState,
        context: &mut AgentContext,
        mut messages: Vec<ModelMessage>,
    ) -> CapabilityResult<Vec<ModelMessage>> {
        context
            .model_config
            .capabilities
            .insert(ModelCapability::ImageUrl);
        let Some(ModelMessage::Request(request)) = messages.last_mut() else {
            panic!("last message should be a request");
        };
        request.parts.push(ModelRequestPart::UserPrompt {
            content: vec![ContentPart::Binary {
                data: tiny_png_bytes(1, 1),
                media_type: "image/png".to_string(),
            }],
            name: None,
            metadata: serde_json::Map::new(),
        });
        Ok(messages)
    }
}

#[async_trait]
impl AgentCapability for FailingRunCompleteCapability {
    fn spec(&self) -> CapabilitySpec {
        CapabilitySpec::new("test.failing_run_complete")
    }

    async fn on_run_complete(&self, _state: &mut AgentRunState) -> CapabilityResult<()> {
        Err(CapabilityError::Failed(
            "run-complete hook failed".to_string(),
        ))
    }
}

#[async_trait]
impl AgentCapability for CustomFilterCapability {
    fn spec(&self) -> CapabilitySpec {
        CapabilitySpec::new("test.custom_filter_capability").with_ordering(
            CapabilityOrdering::default().after("starweaver.filter.reasoning_normalize"),
        )
    }

    async fn prepare_model_messages_with_context(
        &self,
        _state: &mut AgentRunState,
        _context: &mut AgentContext,
        mut messages: Vec<ModelMessage>,
    ) -> CapabilityResult<Vec<ModelMessage>> {
        let Some(ModelMessage::Request(request)) = messages.last_mut() else {
            panic!("last message should be a request");
        };
        request.metadata.insert(
            "custom_filter_capability".to_string(),
            serde_json::json!(true),
        );
        Ok(messages)
    }
}

#[derive(Clone)]
struct LoopingToolModel {
    calls: Arc<Mutex<usize>>,
    loop_until: usize,
}

#[async_trait]
impl ModelAdapter for LoopingToolModel {
    fn model_name(&self) -> &'static str {
        "looping-tool"
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
        let mut calls = self.calls.lock().unwrap();
        *calls += 1;
        if *calls <= self.loop_until {
            Ok(ModelResponse {
                parts: vec![ModelResponsePart::ToolCall(ToolCallPart {
                    id: format!("call_{}", *calls),
                    name: "continue_loop".to_string(),
                    arguments: serde_json::json!({"iteration": *calls}).into(),
                })],
                ..ModelResponse::text("")
            })
        } else {
            Ok(ModelResponse::text("done"))
        }
    }
}

#[async_trait]
impl ModelAdapter for MessageCaptureModel {
    fn model_name(&self) -> &'static str {
        "message-capture"
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
        messages: Vec<ModelMessage>,
        _settings: Option<ModelSettings>,
        _params: ModelRequestParameters,
        _context: ModelRequestContext,
    ) -> Result<ModelResponse, ModelError> {
        self.captured_messages.lock().unwrap().push(messages);
        Ok(ModelResponse::text("captured"))
    }
}

#[async_trait]
impl ModelAdapter for CaptureModel {
    fn model_name(&self) -> &'static str {
        "capture"
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
        self.captured_params.lock().unwrap().push(params);
        Ok(ModelResponse::text(r#"{"answer":"ok"}"#))
    }
}

#[tokio::test]
async fn builder_default_policy_allows_more_than_sixteen_model_steps() {
    let calls = Arc::new(Mutex::new(0));
    let model = Arc::new(LoopingToolModel {
        calls: Arc::clone(&calls),
        loop_until: 17,
    });
    let tool = FunctionTool::new(
        "continue_loop",
        Some("Continue the loop".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
    );

    let result = AgentBuilder::new(model)
        .tool(Arc::new(tool))
        .build()
        .run("loop past old default")
        .await
        .unwrap();

    assert_eq!(result.output, "done");
    assert_eq!(*calls.lock().unwrap(), 18);
}

#[tokio::test]
async fn builder_tool_execution_hook_wraps_runtime_tool_calls() {
    let calls = Arc::new(Mutex::new(0));
    let captured_args = Arc::new(Mutex::new(Vec::new()));
    let hook_order = Arc::new(Mutex::new(Vec::new()));
    let model = Arc::new(LoopingToolModel {
        calls: Arc::clone(&calls),
        loop_until: 1,
    });
    let captured_args_for_tool = captured_args.clone();
    let tool = FunctionTool::new(
        "continue_loop",
        Some("Continue the loop".to_string()),
        serde_json::json!({"type": "object"}),
        move |_ctx: ToolContext, args: serde_json::Value| {
            let captured_args = captured_args_for_tool.clone();
            async move {
                captured_args.lock().unwrap().push(args.clone());
                Ok(ToolResult::new(args))
            }
        },
    );

    let result = AgentBuilder::new(model)
        .tool(Arc::new(tool))
        .tool_execution_hook(
            "continue_loop",
            Arc::new(BuilderToolHook {
                order: hook_order.clone(),
            }),
        )
        .build()
        .run("loop with hook")
        .await
        .unwrap();

    assert_eq!(result.output, "done");
    assert_eq!(*calls.lock().unwrap(), 2);
    assert_eq!(captured_args.lock().unwrap()[0]["builder_hook"], true);
    assert_eq!(*hook_order.lock().unwrap(), vec!["before".to_string()]);
}

#[tokio::test]
async fn builder_creates_reusable_agent_with_tools() {
    let model = Arc::new(CaptureModel {
        captured_params: Arc::new(Mutex::new(Vec::new())),
    });
    let tool = FunctionTool::new(
        "echo",
        Some("Echo input".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
    );
    let tools = ToolRegistry::new().with_tool(Arc::new(tool));

    let agent = AgentBuilder::new(model.clone())
        .instruction("Be concise")
        .output_schema(OutputSchema::new(
            "answer",
            serde_json::json!({"type": "object", "required": ["answer"]}),
        ))
        .usage_limits(UsageLimits::new().with_request_limit(1))
        .tool_registry(tools)
        .build();

    let result = agent.run("hello").await.unwrap();

    assert_eq!(result.output, r#"{"answer":"ok"}"#);
    assert_eq!(result.structured_output.unwrap()["answer"], "ok");
    let params = model.captured_params.lock().unwrap()[0].clone();
    assert_eq!(params.tools.len(), 1);
    assert_eq!(params.tools[0].name, "echo");
    assert_eq!(params.output_schema.unwrap()["name"], "answer");
}

#[tokio::test]
async fn builder_installs_default_filter_capabilities_before_custom_capabilities() {
    let model = FunctionModel::new(
        |messages: Vec<ModelMessage>,
         _settings: Option<ModelSettings>,
         _info: FunctionModelInfo| {
            let Some(ModelMessage::Request(request)) = messages.last() else {
                panic!("last processed message should be a request");
            };
            let order = request
                .metadata
                .get("starweaver_filter_order")
                .and_then(serde_json::Value::as_array)
                .expect("default filters should record their order");
            let observed = order
                .iter()
                .filter_map(serde_json::Value::as_str)
                .collect::<Vec<_>>();
            assert_eq!(observed, DEFAULT_FILTER_ORDER);
            assert_eq!(request.metadata["custom_filter_capability"], true);
            Ok(ModelResponse::text("filters ok"))
        },
    );
    let result = AgentBuilder::new(Arc::new(model))
        .capability(Arc::new(CustomFilterCapability))
        .build()
        .run("hello")
        .await
        .unwrap();

    assert_eq!(result.output, "filters ok");
}

#[tokio::test]
async fn builder_default_media_upload_filter_uses_configured_uploader_without_duplicate_id() {
    let model = FunctionModel::new(
        |messages: Vec<ModelMessage>,
         _settings: Option<ModelSettings>,
         _info: FunctionModelInfo| {
            let content = messages
                .iter()
                .rev()
                .find_map(|message| match message {
                    ModelMessage::Request(request) => {
                        request.parts.iter().rev().find_map(|part| match part {
                            ModelRequestPart::UserPrompt { content, .. } => Some(content),
                            ModelRequestPart::SystemPrompt { .. }
                            | ModelRequestPart::Instruction { .. }
                            | ModelRequestPart::ToolReturn(_)
                            | ModelRequestPart::RetryPrompt { .. } => None,
                        })
                    }
                    ModelMessage::Response(_) => None,
                })
                .expect("uploaded content");
            assert!(content.iter().any(|part| matches!(
                part,
                ContentPart::ResourceRef { uri, media_type, resource_type, .. }
                    if uri == "resource://builder-upload/image"
                        && media_type == "image/png"
                        && resource_type == "image"
            )));
            let order = messages
                .iter()
                .rev()
                .find_map(|message| match message {
                    ModelMessage::Request(request) => {
                        request.metadata.get("starweaver_filter_order")
                    }
                    ModelMessage::Response(_) => None,
                })
                .and_then(serde_json::Value::as_array)
                .expect("filter order");
            assert_eq!(
                order
                    .iter()
                    .filter(|name| name.as_str() == Some("media_upload"))
                    .count(),
                1
            );
            Ok(ModelResponse::text("uploaded"))
        },
    );

    let result = AgentBuilder::new(Arc::new(model))
        .media_uploader(Arc::new(FakeUploader))
        .capability(Arc::new(InlineImageCapability))
        .build()
        .run("upload inline image")
        .await
        .unwrap();

    assert_eq!(result.output, "uploaded");
}

#[tokio::test]
async fn builder_default_media_upload_filter_keeps_original_media_on_upload_failure() {
    let model = FunctionModel::new(
        |messages: Vec<ModelMessage>,
         _settings: Option<ModelSettings>,
         _info: FunctionModelInfo| {
            let request = messages
                .iter()
                .rev()
                .find_map(|message| match message {
                    ModelMessage::Request(request) => Some(request),
                    ModelMessage::Response(_) => None,
                })
                .expect("latest request");
            let content = request
                .parts
                .iter()
                .rev()
                .find_map(|part| match part {
                    ModelRequestPart::UserPrompt { content, .. } => Some(content),
                    ModelRequestPart::SystemPrompt { .. }
                    | ModelRequestPart::Instruction { .. }
                    | ModelRequestPart::ToolReturn(_)
                    | ModelRequestPart::RetryPrompt { .. } => None,
                })
                .expect("media content");
            assert!(content.iter().any(|part| matches!(
                part,
                ContentPart::Binary { media_type, .. } if media_type == "image/png"
            )));
            assert_eq!(
                request.metadata["starweaver_media_upload_failures"][0],
                "upload unavailable"
            );
            Ok(ModelResponse::text("fallback kept"))
        },
    );

    let result = AgentBuilder::new(Arc::new(model))
        .media_uploader(Arc::new(FailingUploader))
        .capability(Arc::new(InlineImageCapability))
        .build()
        .run("upload inline image")
        .await
        .unwrap();

    assert_eq!(result.output, "fallback kept");
}

#[tokio::test]
async fn builder_derives_model_context_capabilities_from_profile_without_overriding_explicit_config()
 {
    let captured_configs = Arc::new(Mutex::new(Vec::new()));
    let derived_model = FunctionModel::new(
        |_messages: Vec<ModelMessage>,
         _settings: Option<ModelSettings>,
         _info: FunctionModelInfo| { Ok(ModelResponse::text("ok")) },
    )
    .with_profile(ModelProfile::for_protocol(
        ProtocolFamily::GeminiGenerateContent,
    ));

    let result = AgentBuilder::new(Arc::new(derived_model))
        .capability(Arc::new(ModelConfigCaptureCapability {
            captured_configs: Arc::clone(&captured_configs),
        }))
        .build()
        .run("hello")
        .await
        .unwrap();
    assert_eq!(result.output, "ok");
    let derived = captured_configs.lock().unwrap()[0].clone();
    assert!(derived.capabilities.contains(&ModelCapability::Vision));
    assert!(
        derived
            .capabilities
            .contains(&ModelCapability::VideoUnderstanding)
    );
    assert!(!derived.capabilities.contains(&ModelCapability::ImageUrl));
    assert!(!derived.capabilities.contains(&ModelCapability::VideoUrl));
    assert!(
        derived
            .capabilities
            .contains(&ModelCapability::AudioUnderstanding)
    );
    assert!(
        derived
            .capabilities
            .contains(&ModelCapability::DocumentUnderstanding)
    );

    let captured_configs = Arc::new(Mutex::new(Vec::new()));
    let explicit_model = FunctionModel::new(
        |_messages: Vec<ModelMessage>,
         _settings: Option<ModelSettings>,
         _info: FunctionModelInfo| { Ok(ModelResponse::text("ok")) },
    )
    .with_profile(ModelProfile::for_protocol(
        ProtocolFamily::GeminiGenerateContent,
    ));
    let mut explicit_config = ModelConfig::default();
    explicit_config.capabilities.insert(ModelCapability::Vision);

    let result = AgentBuilder::new(Arc::new(explicit_model))
        .model_config(explicit_config)
        .capability(Arc::new(ModelConfigCaptureCapability {
            captured_configs: Arc::clone(&captured_configs),
        }))
        .build()
        .run("hello")
        .await
        .unwrap();
    assert_eq!(result.output, "ok");
    let explicit = captured_configs.lock().unwrap()[0].clone();
    assert_eq!(explicit.capabilities.len(), 1);
    assert!(explicit.capabilities.contains(&ModelCapability::Vision));
}

#[tokio::test]
async fn builder_persists_environment_and_runtime_context_for_prefix_stability() {
    let captured_messages = Arc::new(Mutex::new(Vec::new()));
    let model = Arc::new(MessageCaptureModel {
        captured_messages: Arc::clone(&captured_messages),
    });
    let provider = Arc::new(
        VirtualEnvironmentProvider::new("test-env")
            .with_file("README.md", "workspace")
            .with_policy(EnvironmentPolicy {
                files: FilePolicy::read_only(),
                shell: ShellPolicy::default(),
            }),
    );
    let mut context = AgentContext::default();
    attach_environment(&mut context, provider);

    let result = AgentBuilder::new(model)
        .build()
        .run_with_context("inspect workspace", &mut context)
        .await
        .unwrap();

    assert_eq!(result.output, "captured");
    let captured = captured_messages.lock().unwrap();
    let ModelMessage::Request(request) = captured[0].last().unwrap() else {
        panic!("expected latest request");
    };
    assert!(matches!(
        &request.parts[0],
        ModelRequestPart::UserPrompt { content, metadata, .. }
            if metadata.get(CONTEXT_ORIGIN_METADATA)
                == Some(&serde_json::json!(CONTEXT_ORIGIN_RUNTIME_CONTEXT))
                && matches!(&content[0], starweaver_model::ContentPart::Text { text } if text.contains("<runtime-context>"))
    ));
    assert!(matches!(
        &request.parts[1],
        ModelRequestPart::UserPrompt { content, metadata, .. }
            if metadata.get(CONTEXT_ORIGIN_METADATA)
                == Some(&serde_json::json!(CONTEXT_ORIGIN_ENVIRONMENT_CONTEXT))
                && matches!(&content[0], starweaver_model::ContentPart::Text { text } if text.contains("<environment-context>"))
    ));
    assert!(matches!(
        &request.parts[2],
        ModelRequestPart::UserPrompt { content, metadata, .. }
            if metadata.get(CONTEXT_ORIGIN_METADATA).is_none()
                && matches!(&content[0], starweaver_model::ContentPart::Text { text } if text == "inspect workspace")
    ));

    let durable_history =
        serde_json::to_string(&context.message_history).expect("message history should serialize");
    assert!(durable_history.contains("<environment-context>"));
    assert!(!durable_history.contains("<runtime-context>"));
}

#[tokio::test]
async fn multi_run_session_preserves_previous_model_request_prefix() {
    let captured_messages = Arc::new(Mutex::new(Vec::new()));
    let model = Arc::new(MessageCaptureModel {
        captured_messages: Arc::clone(&captured_messages),
    });
    let provider = Arc::new(
        VirtualEnvironmentProvider::new("test-env")
            .with_file("README.md", "workspace")
            .with_policy(EnvironmentPolicy {
                files: FilePolicy::read_only(),
                shell: ShellPolicy::default(),
            }),
    );
    let agent = AgentBuilder::new(model).build();
    let mut session = AgentSession::new(agent);
    attach_environment(session.context_mut(), provider);

    let first = session.run("inspect workspace").await.unwrap();
    assert_eq!(first.output, "captured");
    let second = session.run("continue").await.unwrap();
    assert_eq!(second.output, "captured");

    let captured = captured_messages.lock().unwrap();
    assert_eq!(captured.len(), 2);
    assert_eq!(captured[0].len(), 1);
    assert!(captured[1].len() >= 3);
    assert_eq!(
        context_origin_count(&captured[1], CONTEXT_ORIGIN_ENVIRONMENT_CONTEXT),
        1
    );
    assert_eq!(
        context_origin_count(&captured[1], CONTEXT_ORIGIN_RUNTIME_CONTEXT),
        1
    );
    let first_history = serde_json::to_string(&captured[1][0]).unwrap();
    assert!(first_history.contains("<environment-context>"));
    assert!(!first_history.contains("<runtime-context>"));

    let first_wire =
        OpenAiResponsesAdapter::build_request("gpt-5.5", &captured[0], None, &[], &[]).unwrap();
    let second_wire =
        OpenAiResponsesAdapter::build_request("gpt-5.5", &captured[1], None, &[], &[]).unwrap();
    let first_input = first_wire["input"].as_array().unwrap();
    let second_input = second_wire["input"].as_array().unwrap();
    assert!(second_input.len() > first_input.len());
    assert_eq!(first_input[1], second_input[0]);
    assert_eq!(first_input[2], second_input[1]);
}

#[tokio::test]
async fn summarized_session_preserves_next_request_stable_prefix_before_runtime_context() {
    let model = TestModel::with_responses(vec![
        tool_call_response(
            "call_summarize",
            "summarize",
            serde_json::json!({
                "content": "## Current State\nSummarized prior work.\n\n## Next Step\nContinue implementation.",
                "auto_load_files": []
            }),
        ),
        ModelResponse::text("handoff complete"),
        ModelResponse::text("continued"),
    ]);
    let mut session = AgentSession::new(
        AgentBuilder::new(Arc::new(model.clone()))
            .toolset(&context_tools())
            .build(),
    );

    let summarized = session.run("summarize now").await.unwrap();
    assert_eq!(summarized.output, "handoff complete");
    let continued = session.run("continue after summary").await.unwrap();
    assert_eq!(continued.output, "continued");

    let captured = model.captured_messages();
    assert_eq!(captured.len(), 3);
    let handoff_wire =
        OpenAiResponsesAdapter::build_request("gpt-5.5", &captured[1], None, &[], &[]).unwrap();
    let next_wire =
        OpenAiResponsesAdapter::build_request("gpt-5.5", &captured[2], None, &[], &[]).unwrap();
    let handoff_input = handoff_wire["input"].as_array().unwrap();
    let next_input = next_wire["input"].as_array().unwrap();
    let stable_prefix_len = input_index_containing(next_input, "<context-restored>")
        .expect("next request should retain handoff restore marker")
        + 1;

    assert_eq!(
        &handoff_input[..stable_prefix_len],
        &next_input[..stable_prefix_len]
    );
    assert!(
        !handoff_input[..stable_prefix_len]
            .iter()
            .any(|input| input_contains_text(input, "<runtime-context>"))
    );
}

#[tokio::test]
async fn compacted_session_preserves_next_request_stable_prefix_before_runtime_context() {
    let captured_messages = Arc::new(Mutex::new(Vec::new()));
    let main_model = Arc::new(MessageCaptureModel {
        captured_messages: Arc::clone(&captured_messages),
    });
    let compact_model = FunctionModel::new(|_messages, _settings, _info| {
        Ok(ModelResponse::text(
            "## Condensed conversation summary\n\nKeep working from the compact summary.",
        ))
    });
    let mut session = AgentSession::new(
        AgentBuilder::new(main_model)
            .compact_model(Arc::new(compact_model))
            .build(),
    );
    session.context_mut().model_config = ModelConfig {
        context_window: Some(100),
        compact_threshold: PerThousandRatio::from_per_thousand(900),
        ..ModelConfig::default()
    };
    let mut prior_response = ModelResponse::text("large prior response");
    prior_response.usage = Usage {
        requests: 1,
        input_tokens: 90,
        output_tokens: 5,
        total_tokens: 95,
        ..Usage::default()
    };
    session.context_mut().message_history = vec![
        ModelMessage::Request(ModelRequest::user_text("old request")),
        ModelMessage::Response(prior_response),
    ];

    let compacted = session.run("resume after compact").await.unwrap();
    assert_eq!(compacted.output, "captured");
    let after_compact = session.run("continue after compact").await.unwrap();
    assert_eq!(after_compact.output, "captured");

    let captured = captured_messages.lock().unwrap();
    assert_eq!(captured.len(), 2);
    let compact_wire =
        OpenAiResponsesAdapter::build_request("gpt-5.5", &captured[0], None, &[], &[]).unwrap();
    let next_wire =
        OpenAiResponsesAdapter::build_request("gpt-5.5", &captured[1], None, &[], &[]).unwrap();
    let compact_input = compact_wire["input"].as_array().unwrap();
    let next_input = next_wire["input"].as_array().unwrap();
    let stable_prefix_len = input_index_containing(next_input, "<context-restored>")
        .expect("next request should retain compact restore marker")
        + 1;

    assert_eq!(
        &compact_input[..stable_prefix_len],
        &next_input[..stable_prefix_len]
    );
    assert!(
        !compact_input[..stable_prefix_len]
            .iter()
            .any(|input| input_contains_text(input, "<runtime-context>"))
    );
}

fn tiny_png_bytes(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    bytes.extend_from_slice(&13u32.to_be_bytes());
    bytes.extend_from_slice(b"IHDR");
    bytes.extend_from_slice(&width.to_be_bytes());
    bytes.extend_from_slice(&height.to_be_bytes());
    bytes.extend_from_slice(&[8, 2, 0, 0, 0]);
    bytes.extend_from_slice(&0u32.to_be_bytes());
    bytes.extend_from_slice(&0u32.to_be_bytes());
    bytes.extend_from_slice(b"IEND");
    bytes.extend_from_slice(&0u32.to_be_bytes());
    bytes
}

fn context_origin_count(messages: &[ModelMessage], origin: &str) -> usize {
    messages
        .iter()
        .filter_map(|message| match message {
            ModelMessage::Request(request) => Some(request),
            ModelMessage::Response(_) => None,
        })
        .flat_map(|request| request.parts.iter())
        .filter(|part| {
            matches!(
                part,
                ModelRequestPart::UserPrompt { metadata, .. }
                    if metadata.get(CONTEXT_ORIGIN_METADATA) == Some(&serde_json::json!(origin))
            )
        })
        .count()
}

fn input_index_containing(inputs: &[serde_json::Value], needle: &str) -> Option<usize> {
    inputs
        .iter()
        .position(|input| input_contains_text(input, needle))
}

fn input_contains_text(input: &serde_json::Value, needle: &str) -> bool {
    match input {
        serde_json::Value::String(text) => text.contains(needle),
        serde_json::Value::Array(items) => {
            items.iter().any(|item| input_contains_text(item, needle))
        }
        serde_json::Value::Object(map) => {
            map.values().any(|value| input_contains_text(value, needle))
        }
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {
            false
        }
    }
}

#[tokio::test]
async fn builder_agents_support_test_model_override() {
    let agent = AgentBuilder::new(Arc::new(TestModel::with_text("production"))).build();

    let overridden = agent
        .override_config()
        .model(Arc::new(TestModel::with_text("test")))
        .build();

    let result = overridden.run("hello").await.unwrap();

    assert_eq!(result.output, "test");
}

#[tokio::test]
async fn builder_applies_capability_bundle() {
    let model = Arc::new(CaptureModel {
        captured_params: Arc::new(Mutex::new(Vec::new())),
    });
    let tool = FunctionTool::new(
        "bundle_tool",
        Some("Bundle tool".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
    );
    let bundle = StaticCapabilityBundle::new("builder-bundle")
        .with_instruction("Use the builder bundle.")
        .with_tool(Arc::new(tool));

    let result = AgentBuilder::new(model.clone())
        .capability_bundle(Arc::new(bundle))
        .build()
        .run("hello")
        .await
        .unwrap();

    assert_eq!(result.output, r#"{"answer":"ok"}"#);
    let params = model.captured_params.lock().unwrap()[0].clone();
    assert_eq!(params.tools.len(), 1);
    assert_eq!(params.tools[0].name, "bundle_tool");
}

#[tokio::test]
async fn builder_applies_dynamic_instruction() {
    let model = Arc::new(CaptureModel {
        captured_params: Arc::new(Mutex::new(Vec::new())),
    });
    let instruction = FunctionDynamicInstruction::new(|state: AgentRunState| async move {
        Ok(format!("builder dynamic step {}", state.run_step))
    });

    let result = AgentBuilder::new(model)
        .dynamic_instruction(Arc::new(instruction))
        .build()
        .run("hello")
        .await
        .unwrap();

    assert_eq!(result.output, r#"{"answer":"ok"}"#);
}

#[tokio::test]
async fn builder_applies_settings_params_validators_functions_and_toolsets() {
    let model = Arc::new(CaptureModel {
        captured_params: Arc::new(Mutex::new(Vec::new())),
    });
    let tool = Arc::new(FunctionTool::new(
        "extra",
        Some("Extra tool".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
    ));
    let toolset_tool = Arc::new(FunctionTool::new(
        "toolset_extra",
        Some("Toolset extra tool".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
    ));
    let toolset: starweaver_tools::DynToolset =
        Arc::new(StaticToolset::new("extras").with_tool(toolset_tool));
    let mut params = ModelRequestParameters::default();
    params
        .extra_body
        .insert("route".to_string(), serde_json::json!("sdk"));
    let validator =
        FunctionOutputValidator::new(|_state: &mut AgentRunState, output: &OutputValue| {
            let text = output.as_text();
            std::future::ready({
                assert!(text.contains("answer"));
                Ok(())
            })
        });
    let output_function = FunctionOutputFunction::new(
        OutputFunctionDefinition::new("final_answer", serde_json::json!({"type": "object"})),
        |_ctx, args: serde_json::Value| async move { Ok(OutputValue::Json(args)) },
    );

    let result = AgentBuilder::new(model.clone())
        .model_settings(ModelSettings {
            temperature: Some(0.3),
            ..ModelSettings::default()
        })
        .request_params(params)
        .output_validator(Arc::new(validator))
        .output_function(Arc::new(output_function))
        .tool(tool)
        .toolset(&toolset)
        .tool_retries(2)
        .build()
        .run("hello")
        .await
        .unwrap();

    assert_eq!(result.output, r#"{"answer":"ok"}"#);
    let params = model.captured_params.lock().unwrap()[0].clone();
    let tool_names = params
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();
    assert!(tool_names.contains(&"extra"));
    assert!(tool_names.contains(&"toolset_extra"));
    assert!(tool_names.contains(&"final_answer"));
    assert_eq!(params.extra_body["route"], "sdk");
}

#[tokio::test]
async fn builder_prepares_toolsets_with_run_context() {
    let model = Arc::new(CaptureModel {
        captured_params: Arc::new(Mutex::new(Vec::new())),
    });
    let toolset: DynToolset = Arc::new(TenantPreparedToolset);
    let mut context = AgentContext::default();
    context
        .metadata
        .insert("tenant".to_string(), serde_json::json!("alpha"));

    let result = AgentBuilder::new(model.clone())
        .toolset(&toolset)
        .build()
        .run_with_context("hello", &mut context)
        .await
        .unwrap();

    assert_eq!(result.output, r#"{"answer":"ok"}"#);
    let params = model.captured_params.lock().unwrap()[0].clone();
    assert!(params.tools.iter().any(|tool| tool.name == "tenant_echo"));
    assert!(
        params
            .instructions
            .iter()
            .any(|instruction| instruction.text.contains("Use tenant tools."))
    );
    let event = context
        .events
        .events()
        .iter()
        .find(|event| event.kind == TOOLSET_INITIALIZED_EVENT_KIND)
        .expect("toolset lifecycle event");
    assert_eq!(event.payload["name"], "tenant_tools");
    assert_eq!(event.payload["state"], "initialized");
    assert_eq!(event.payload["tool_count"], 1);
}

#[tokio::test]
async fn builder_closes_lifecycle_toolsets_before_run_exit() {
    let lifecycle_calls = Arc::new(Mutex::new(Vec::new()));
    let toolset: DynToolset = Arc::new(RunLifecycleToolset {
        calls: lifecycle_calls.clone(),
    });
    let mut context = AgentContext::default();

    let result = AgentBuilder::new(Arc::new(TestModel::with_text("done")))
        .toolset(&toolset)
        .build()
        .run_with_context("hello", &mut context)
        .await
        .unwrap();

    assert_eq!(result.output, "done");
    assert_eq!(
        *lifecycle_calls.lock().unwrap(),
        vec![
            "enter".to_string(),
            "prepare".to_string(),
            "exit".to_string()
        ]
    );
    let closed_event = context
        .events
        .events()
        .iter()
        .find(|event| event.kind == TOOLSET_CLOSED_EVENT_KIND)
        .expect("toolset close event");
    assert_eq!(closed_event.payload["name"], "run_lifecycle_tools");
    assert_eq!(closed_event.payload["state"], "closed");
    assert!(!context.runtime.lifecycle.entered);
    assert!(context.ended_at.is_some());
}

#[tokio::test]
async fn builder_closes_lifecycle_toolsets_after_model_failure() {
    let lifecycle_calls = Arc::new(Mutex::new(Vec::new()));
    let toolset: DynToolset = Arc::new(RunLifecycleToolset {
        calls: lifecycle_calls.clone(),
    });
    let mut context = AgentContext::default();
    let model = FunctionModel::new(|_messages, _settings, _info| {
        Err(ModelError::Transport("network unavailable".to_string()))
    });

    let result = AgentBuilder::new(Arc::new(model))
        .toolset(&toolset)
        .build()
        .run_with_context("hello", &mut context)
        .await;

    assert!(result.is_err());
    assert_eq!(
        *lifecycle_calls.lock().unwrap(),
        vec![
            "enter".to_string(),
            "prepare".to_string(),
            "exit".to_string()
        ]
    );
    assert!(
        context
            .events
            .events()
            .iter()
            .any(|event| event.kind == TOOLSET_CLOSED_EVENT_KIND)
    );
    assert!(
        context
            .events
            .events()
            .iter()
            .any(|event| event.kind == "run_failed")
    );
    assert!(!context.runtime.lifecycle.entered);
    assert!(context.ended_at.is_some());
}

#[tokio::test]
async fn builder_fallback_cleanup_closes_toolsets_after_run_complete_hook_failure() {
    let lifecycle_calls = Arc::new(Mutex::new(Vec::new()));
    let toolset: DynToolset = Arc::new(RunLifecycleToolset {
        calls: lifecycle_calls.clone(),
    });
    let mut context = AgentContext::default();

    let result = AgentBuilder::new(Arc::new(TestModel::with_text("done")))
        .toolset(&toolset)
        .capability(Arc::new(FailingRunCompleteCapability))
        .build()
        .run_with_context("hello", &mut context)
        .await;

    assert!(result.is_err());
    assert_eq!(
        *lifecycle_calls.lock().unwrap(),
        vec![
            "enter".to_string(),
            "prepare".to_string(),
            "exit".to_string()
        ]
    );
    assert!(!context.runtime.lifecycle.entered);
    assert!(context.ended_at.is_some());
    assert!(context.events.events().iter().any(|event| {
        event.kind == "run_failed"
            && event.payload["error_kind"] == "capability_error"
            && event.payload["message"] == "agent capability failed"
    }));
}

#[test]
fn builder_replaces_subagent_registry_and_policy() {
    let child = Arc::new(AgentBuilder::new(Arc::new(TestModel::with_text("child"))).build());
    let mut registry = starweaver_agent::SubagentRegistry::new();
    registry.insert(starweaver_agent::SubagentConfig::new("child", child));

    let builder = AgentBuilder::new(Arc::new(TestModel::with_text("parent")))
        .subagent_registry(registry)
        .policy(starweaver_agent::AgentRuntimePolicy {
            max_steps: 3,
            output_retries: 2,
            ..starweaver_agent::AgentRuntimePolicy::default()
        });
    let app = builder.build_app();

    assert_eq!(app.subagents().subagents().len(), 1);
    assert!(app.subagents().subagent("child").is_some());
}
