#![allow(
    missing_docs,
    clippy::expect_used,
    clippy::significant_drop_tightening,
    clippy::unwrap_used
)]

use std::sync::{Arc, LazyLock, Mutex};

use async_trait::async_trait;
use starweaver_agent::{
    attach_environment, AgentBuilder, AgentCapability, AgentContext, AgentRunState, AgentSession,
    CapabilityOrdering, CapabilityResult, CapabilitySpec, FunctionDynamicInstruction,
    FunctionModel, FunctionModelInfo, FunctionOutputFunction, FunctionOutputValidator,
    FunctionTool, OutputFunctionDefinition, OutputSchema, OutputValue, StaticCapabilityBundle,
    StaticToolset, TestModel, ToolContext, ToolRegistry, ToolResult, UsageLimits,
    DEFAULT_FILTER_ORDER,
};
use starweaver_environment::{
    EnvironmentPolicy, FilePolicy, ShellPolicy, VirtualEnvironmentProvider,
};
use starweaver_model::{
    providers::openai_responses::OpenAiResponsesAdapter, ModelAdapter, ModelError, ModelMessage,
    ModelProfile, ModelRequestContext, ModelRequestParameters, ModelRequestPart, ModelResponse,
    ModelResponsePart, ModelSettings, ProtocolFamily, ToolCallPart,
    CONTEXT_ORIGIN_ENVIRONMENT_CONTEXT, CONTEXT_ORIGIN_METADATA, CONTEXT_ORIGIN_RUNTIME_CONTEXT,
};

#[derive(Clone)]
struct CaptureModel {
    captured_params: Arc<Mutex<Vec<ModelRequestParameters>>>,
}

struct CustomFilterCapability;

#[derive(Clone)]
struct MessageCaptureModel {
    captured_messages: Arc<Mutex<Vec<Vec<ModelMessage>>>>,
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
            if metadata.get(CONTEXT_ORIGIN_METADATA).is_none()
                && matches!(&content[0], starweaver_model::ContentPart::Text { text } if text == "inspect workspace")
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
            if metadata.get(CONTEXT_ORIGIN_METADATA)
                == Some(&serde_json::json!(CONTEXT_ORIGIN_RUNTIME_CONTEXT))
                && matches!(&content[0], starweaver_model::ContentPart::Text { text } if text.contains("<runtime-context>"))
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
    assert_eq!(first_input[0], second_input[0]);
    assert_eq!(first_input[1], second_input[1]);
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
        });
    let app = builder.build_app();

    assert_eq!(app.subagents().subagents().len(), 1);
    assert!(app.subagents().subagent("child").is_some());
}
