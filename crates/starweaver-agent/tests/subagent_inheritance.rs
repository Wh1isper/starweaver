#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::{Arc, LazyLock, Mutex};

use async_trait::async_trait;
use starweaver_agent::{
    ASK_USER_QUESTION_TOOL_NAME, AgentBuilder, AgentCapability, AgentContext, AgentRunState,
    CapabilityResult, CapabilitySpec, FunctionModel, FunctionTool, StaticCapabilityBundle,
    SubagentCapabilityInheritancePolicy, SubagentConfig, SubagentParentTools, SubagentRegistry,
    SubagentToolInheritanceError, SubagentToolInheritancePolicy, TestModel, ToolContext, ToolError,
    ToolRegistry, ToolResult,
};
use starweaver_core::Metadata;
use starweaver_model::{
    ModelAdapter, ModelError, ModelMessage, ModelProfile, ModelRequest, ModelRequestContext,
    ModelRequestParameters, ModelRequestPart, ModelResponse, ModelSettings, ProtocolFamily,
    tool_call_response,
};

type ReadyToolResult = std::future::Ready<Result<ToolResult, ToolError>>;
type ReadyFunctionTool = FunctionTool<fn(ToolContext, serde_json::Value) -> ReadyToolResult>;

#[derive(Clone)]
struct ToolCaptureModel {
    captured_tools: Arc<Mutex<Vec<Vec<String>>>>,
}

#[async_trait]
impl ModelAdapter for ToolCaptureModel {
    fn model_name(&self) -> &'static str {
        "tool-capture"
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
        self.captured_tools
            .lock()
            .unwrap()
            .push(params.tools.iter().map(|tool| tool.name.clone()).collect());
        Ok(ModelResponse::text("child done"))
    }
}

#[test]
fn subagent_tool_inheritance_resolves_required_optional_denied_and_auto_tools() {
    let parent = ToolRegistry::new()
        .with_tool(auto_tool("task_list"))
        .with_tool(plain_tool("search"))
        .with_tool(plain_tool("shell_exec"));
    let policy = SubagentToolInheritancePolicy::new(
        vec!["task_list".to_string()],
        vec!["search".to_string()],
    )
    .with_denied_tools(vec!["shell_exec".to_string()]);

    let inherited = policy.resolve(&parent).unwrap();

    assert_eq!(inherited.names(), vec!["search", "task_list"]);
    assert!(matches!(
        SubagentToolInheritancePolicy::new(vec!["missing".to_string()], vec![])
            .resolve(&parent)
            .err()
            .unwrap(),
        SubagentToolInheritanceError::MissingRequiredTool(name) if name == "missing"
    ));
    assert!(matches!(
        SubagentToolInheritancePolicy::new(vec!["search".to_string()], vec![])
            .with_denied_tools(vec!["search".to_string()])
            .resolve(&parent)
            .err()
            .unwrap(),
        SubagentToolInheritanceError::DeniedRequiredTool(name) if name == "search"
    ));
}

#[test]
fn subagents_never_inherit_main_agent_user_input_tool() {
    let parent = ToolRegistry::new().with_tool(auto_tool(ASK_USER_QUESTION_TOOL_NAME));

    let inherited_all = SubagentToolInheritancePolicy::default()
        .with_inherit_all_when_empty(true)
        .resolve(&parent)
        .unwrap();
    assert!(
        !inherited_all
            .names()
            .iter()
            .any(|name| name == ASK_USER_QUESTION_TOOL_NAME)
    );

    let optional = SubagentToolInheritancePolicy::new(
        Vec::new(),
        vec![ASK_USER_QUESTION_TOOL_NAME.to_string()],
    )
    .resolve(&parent)
    .unwrap();
    assert!(
        !optional
            .names()
            .iter()
            .any(|name| name == ASK_USER_QUESTION_TOOL_NAME)
    );

    assert!(matches!(
        SubagentToolInheritancePolicy::new(
            vec![ASK_USER_QUESTION_TOOL_NAME.to_string()],
            Vec::new(),
        )
        .resolve(&parent),
        Err(SubagentToolInheritanceError::DeniedRequiredTool(name))
            if name == ASK_USER_QUESTION_TOOL_NAME
    ));
}

#[tokio::test]
async fn subagent_final_model_tools_exclude_child_owned_user_input_tool() {
    let captured_tools = Arc::new(Mutex::new(Vec::new()));
    let child = Arc::new(
        AgentBuilder::new(Arc::new(ToolCaptureModel {
            captured_tools: Arc::clone(&captured_tools),
        }))
        .tool(plain_tool(ASK_USER_QUESTION_TOOL_NAME))
        .tool(plain_tool("child_tool"))
        .build(),
    );
    let registry = SubagentRegistry::new().with_subagent(SubagentConfig::new("child", child));
    let mut context = AgentContext::default();

    let result = registry
        .delegate("child", "work", &mut context)
        .await
        .unwrap();

    assert_eq!(result.output, "child done");
    let requests = captured_tools.lock().unwrap().clone();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].iter().any(|name| name == "child_tool"));
    assert!(
        !requests[0]
            .iter()
            .any(|name| name == ASK_USER_QUESTION_TOOL_NAME)
    );
}

#[tokio::test]
async fn subagent_delegation_reports_denied_inherited_required_tool() {
    let child = Arc::new(AgentBuilder::new(Arc::new(TestModel::with_text("unused"))).build());
    let registry = SubagentRegistry::new().with_subagent(
        SubagentConfig::new("child", child).with_tool_inheritance(
            SubagentToolInheritancePolicy::new(vec!["task_list".to_string()], vec![])
                .with_denied_tools(vec!["task_list".to_string()]),
        ),
    );
    let mut context = AgentContext::default();
    context.dependencies.insert(SubagentParentTools(
        ToolRegistry::new().with_tool(auto_tool("task_list")),
    ));

    let error = registry
        .delegate("child", "try inherited denied tool", &mut context)
        .await
        .unwrap_err();

    assert!(error.to_string().contains("required inherited tool"));
    let failed = context
        .events
        .events()
        .iter()
        .find(|event| event.kind == "subagent_failed")
        .unwrap();
    assert_eq!(
        failed.payload["metadata"]["error_kind"],
        "denied_required_tool"
    );
    assert_eq!(failed.payload["metadata"]["tool_name"], "task_list");
}

#[tokio::test]
async fn subagent_delegation_inherits_parent_tools_into_child_run() {
    let child_model = TestModel::with_responses(vec![
        tool_call_response("call", "task_list", serde_json::json!({})),
        ModelResponse::text("child done"),
    ]);
    let child = Arc::new(AgentBuilder::new(Arc::new(child_model)).build());
    let registry = Arc::new(SubagentRegistry::new().with_subagent(
        SubagentConfig::new("child", child).with_tool_inheritance(
            SubagentToolInheritancePolicy::new(vec!["task_list".to_string()], vec![]),
        ),
    ));
    let parent_model = TestModel::with_responses(vec![
        tool_call_response(
            "delegate",
            "delegate",
            serde_json::json!({"subagent_name": "child", "prompt": "work"}),
        ),
        ModelResponse::text("parent done"),
    ]);
    let agent = AgentBuilder::new(Arc::new(parent_model))
        .tool(auto_tool("task_list"))
        .tool(registry.delegate_tool())
        .subagent_registry((*registry).clone())
        .build();
    let mut context = AgentContext::default();

    let result = agent.run_with_context("start", &mut context).await.unwrap();

    assert_eq!(result.output, "parent done");
    assert!(
        context
            .events
            .events()
            .iter()
            .any(|event| event.kind == "subagent_completed")
    );
}

#[tokio::test]
async fn subagent_delegation_inherits_parent_capability_bundles_when_declared() {
    let child_model = TestModel::with_responses(vec![
        tool_call_response("bundle", "bundle_tool", serde_json::json!({"ok": true})),
        ModelResponse::text("child used bundle"),
    ]);
    let child = Arc::new(AgentBuilder::new(Arc::new(child_model)).build());
    let parent_model = TestModel::with_responses(vec![
        tool_call_response(
            "delegate",
            "delegate",
            serde_json::json!({"subagent_name": "child", "prompt": "work"}),
        ),
        ModelResponse::text("parent done"),
    ]);
    let inherited_bundle =
        StaticCapabilityBundle::new("parent-bundle").with_tool(plain_tool("bundle_tool"));
    let agent = AgentBuilder::new(Arc::new(parent_model))
        .capability_bundle(Arc::new(inherited_bundle))
        .subagent(
            SubagentConfig::new("child", child).with_capability_inheritance(
                SubagentCapabilityInheritancePolicy::default().with_capability_bundles(true),
            ),
        )
        .build();
    let mut context = AgentContext::default();

    let result = agent.run_with_context("start", &mut context).await.unwrap();

    assert_eq!(result.output, "parent done");
    assert!(
        context
            .events
            .events()
            .iter()
            .any(|event| event.kind == "subagent_completed")
    );
}

#[tokio::test]
async fn subagent_delegation_inherits_parent_hooks_when_declared() {
    let child_model = FunctionModel::new(|messages, _settings, _context| {
        let inherited = messages.iter().any(|message| match message {
            ModelMessage::Request(request) => request.parts.iter().any(|part| {
                matches!(
                    part,
                    ModelRequestPart::SystemPrompt { text, .. }
                        if text == "inherited hook marker"
                )
            }),
            ModelMessage::Response(_) => false,
        });
        Ok(ModelResponse::text(if inherited {
            "child inherited hook"
        } else {
            "child missing hook"
        }))
    });
    let child = Arc::new(AgentBuilder::new(Arc::new(child_model)).build());
    let parent_model = TestModel::with_responses(vec![
        tool_call_response(
            "delegate",
            "delegate",
            serde_json::json!({"subagent_name": "child", "prompt": "work"}),
        ),
        ModelResponse::text("parent done"),
    ]);
    let agent = AgentBuilder::new(Arc::new(parent_model))
        .capability(Arc::new(RunStartMarker))
        .subagent(
            SubagentConfig::new("child", child).with_capability_inheritance(
                SubagentCapabilityInheritancePolicy::default().with_hooks(true),
            ),
        )
        .build();
    let mut context = AgentContext::default();

    let result = agent.run_with_context("start", &mut context).await.unwrap();

    assert_eq!(result.output, "parent done");
    assert!(context.subagent_history.values().flatten().any(|message| {
        matches!(
            message,
            ModelMessage::Response(response) if response.text_output() == "child inherited hook"
        )
    }));
}

struct RunStartMarker;

#[async_trait]
impl AgentCapability for RunStartMarker {
    fn spec(&self) -> CapabilitySpec {
        CapabilitySpec::new("run-start-marker")
    }

    async fn prepare_model_messages(
        &self,
        _state: &mut AgentRunState,
        mut messages: Vec<ModelMessage>,
    ) -> CapabilityResult<Vec<ModelMessage>> {
        messages.insert(
            0,
            ModelMessage::Request(ModelRequest {
                parts: vec![ModelRequestPart::SystemPrompt {
                    text: "inherited hook marker".to_string(),
                    metadata: Metadata::default(),
                }],
                timestamp: None,
                instructions: None,
                run_id: None,
                conversation_id: None,
                metadata: Metadata::default(),
            }),
        );
        Ok(messages)
    }

    async fn on_run_start_with_context(
        &self,
        state: &mut AgentRunState,
        _context: &mut AgentContext,
    ) -> CapabilityResult<()> {
        self.on_run_start(state).await
    }
}

fn plain_tool(name: &'static str) -> Arc<ReadyFunctionTool> {
    Arc::new(FunctionTool::new(
        name,
        Some(format!("{name} tool")),
        serde_json::json!({"type": "object"}),
        ready_tool as fn(ToolContext, serde_json::Value) -> ReadyToolResult,
    ))
}

fn auto_tool(name: &'static str) -> Arc<ReadyFunctionTool> {
    let mut metadata = Metadata::default();
    metadata.insert("auto_inherit".to_string(), serde_json::json!(true));
    Arc::new(
        FunctionTool::new(
            name,
            Some(format!("{name} tool")),
            serde_json::json!({"type": "object"}),
            ready_tool as fn(ToolContext, serde_json::Value) -> ReadyToolResult,
        )
        .with_metadata(metadata),
    )
}

fn ready_tool(_context: ToolContext, arguments: serde_json::Value) -> ReadyToolResult {
    std::future::ready(Ok(ToolResult::new(arguments)))
}
