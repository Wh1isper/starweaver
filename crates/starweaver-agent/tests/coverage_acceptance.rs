#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use starweaver_agent::{
    AgentBuilder, AgentContext, AgentRunOptions, AgentSession, EnvironmentHandle, FunctionTool,
    StaticToolset, SubagentConfig, SubagentParentTools, SubagentRegistry,
    SubagentToolInheritancePolicy, TestModel, ToolContext, ToolRegistry, ToolResult,
};
use starweaver_core::{ConversationId, RunId, TaskId, Usage};
use starweaver_model::{
    ModelAdapter, ModelError, ModelMessage, ModelProfile, ModelRequestContext,
    ModelRequestParameters, ModelResponse, ModelResponseStreamEvent, ModelSettings, PartDelta,
    PartEnd, PartStart, ProtocolFamily,
};

#[derive(Clone, Default)]
struct CapturedStreamRequest {
    messages: Vec<ModelMessage>,
    settings: Option<ModelSettings>,
    params: ModelRequestParameters,
}

#[derive(Clone)]
struct StreamingCaptureModel {
    captured: Arc<Mutex<Vec<CapturedStreamRequest>>>,
}

#[async_trait]
impl ModelAdapter for StreamingCaptureModel {
    fn model_name(&self) -> &'static str {
        "streaming-capture"
    }

    fn provider_name(&self) -> Option<&'static str> {
        Some("test")
    }

    fn profile(&self) -> &ModelProfile {
        static PROFILE: ModelProfile =
            ModelProfile::for_protocol(ProtocolFamily::OpenAiChatCompletions);
        &PROFILE
    }

    fn default_settings(&self) -> Option<&ModelSettings> {
        None
    }

    async fn request(
        &self,
        messages: Vec<ModelMessage>,
        settings: Option<ModelSettings>,
        params: ModelRequestParameters,
        context: ModelRequestContext,
    ) -> Result<ModelResponse, ModelError> {
        self.request_stream(messages, settings, params, context)
            .await
            .map(|_| ModelResponse::text("streamed"))
    }

    async fn request_stream(
        &self,
        messages: Vec<ModelMessage>,
        settings: Option<ModelSettings>,
        params: ModelRequestParameters,
        _context: ModelRequestContext,
    ) -> Result<Vec<ModelResponseStreamEvent>, ModelError> {
        self.captured.lock().unwrap().push(CapturedStreamRequest {
            messages,
            settings,
            params,
        });
        Ok(vec![
            ModelResponseStreamEvent::PartStart(PartStart {
                index: 0,
                part_kind: "text".to_string(),
            }),
            ModelResponseStreamEvent::PartDelta(PartDelta {
                index: 0,
                delta: "stream".to_string(),
            }),
            ModelResponseStreamEvent::PartEnd(PartEnd { index: 0 }),
            ModelResponseStreamEvent::FinalResult(Box::new(ModelResponse::text("streamed"))),
        ])
    }
}

#[tokio::test]
async fn session_run_stream_with_options_applies_all_option_builders() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let model = Arc::new(StreamingCaptureModel {
        captured: captured.clone(),
    });
    let direct_tool = Arc::new(FunctionTool::new(
        "direct_tool",
        Some("Direct run tool".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
    ));
    let registry_tool = Arc::new(FunctionTool::new(
        "registry_tool",
        Some("Registry run tool".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
    ));
    let toolset_tool = Arc::new(FunctionTool::new(
        "toolset_tool",
        Some("Toolset run tool".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
    ));
    let extra_toolset_tool = Arc::new(FunctionTool::new(
        "extra_toolset_tool",
        Some("Extra toolset run tool".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
    ));
    let toolset = Arc::new(StaticToolset::new("run-tools").with_tool(toolset_tool));
    let extra_toolset =
        Arc::new(StaticToolset::new("extra-run-tools").with_tool(extra_toolset_tool));
    let registry = ToolRegistry::new().with_tool(registry_tool);
    let mut params = ModelRequestParameters::default();
    params
        .extra_body
        .insert("route".to_string(), serde_json::json!("stream"));
    let mut session = AgentSession::new(AgentBuilder::new(model).build());

    let stream = session
        .run_stream_with_options(
            "hello",
            AgentRunOptions::new()
                .instruction("stream run instruction")
                .model_settings(ModelSettings {
                    temperature: Some(0.4),
                    ..ModelSettings::default()
                })
                .request_params(params)
                .tool(direct_tool)
                .toolset(&(toolset as starweaver_agent::DynToolset))
                .toolsets(vec![extra_toolset as starweaver_agent::DynToolset])
                .append_tool_registry(&registry),
        )
        .await
        .unwrap();

    assert_eq!(stream.result.output, "streamed");
    assert!(stream.events.iter().any(|record| matches!(
        record.event,
        starweaver_agent::AgentStreamEvent::ModelStream { .. }
    )));
    let captured = captured.lock().unwrap().clone();
    assert_eq!(captured.len(), 1);
    let tool_names = captured[0]
        .params
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();
    assert!(tool_names.contains(&"direct_tool"));
    assert!(tool_names.contains(&"registry_tool"));
    assert!(tool_names.contains(&"toolset_tool"));
    assert!(tool_names.contains(&"extra_toolset_tool"));
    assert_eq!(captured[0].params.extra_body["route"], "stream");
    assert_eq!(
        captured[0].settings.as_ref().unwrap().temperature,
        Some(0.4)
    );
    assert!(format!("{:?}", captured[0].messages).contains("stream run instruction"));
}

#[tokio::test]
async fn session_environment_helpers_attach_provider_dependency() {
    let provider = Arc::new(starweaver_environment::VirtualEnvironmentProvider::new(
        "virtual",
    ));
    let mut session =
        AgentSession::new(AgentBuilder::new(Arc::new(TestModel::with_text("ok"))).build())
            .with_environment(provider.clone());

    assert!(session
        .context()
        .dependency::<EnvironmentHandle>()
        .is_some());

    session.set_environment(provider);
    let result = session.run("hello").await.unwrap();

    assert_eq!(result.output, "ok");
    assert!(session
        .context()
        .dependency::<EnvironmentHandle>()
        .is_some());
}

#[test]
fn subagent_tool_inheritance_covers_builder_and_error_paths() {
    let required = Arc::new(FunctionTool::new(
        "required_tool",
        Some("Required tool".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
    ));
    let auto_metadata = serde_json::json!({"auto_inherit": true})
        .as_object()
        .unwrap()
        .clone();
    let delegate = Arc::new(
        FunctionTool::new(
            "delegate",
            Some("Nested delegate".to_string()),
            serde_json::json!({"type": "object"}),
            |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
        )
        .with_metadata(auto_metadata.clone()),
    ) as starweaver_agent::DynTool;
    let subagent_info = Arc::new(
        FunctionTool::new(
            "subagent_info",
            Some("Nested info".to_string()),
            serde_json::json!({"type": "object"}),
            |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
        )
        .with_metadata(auto_metadata),
    ) as starweaver_agent::DynTool;
    let parent = ToolRegistry::new()
        .with_tool(required)
        .with_tool(delegate)
        .with_tool(subagent_info);

    let inherited = SubagentToolInheritancePolicy::new(
        vec!["required_tool".to_string()],
        vec![
            "delegate".to_string(),
            "subagent_info".to_string(),
            "missing_optional".to_string(),
        ],
    )
    .without_auto_inherit()
    .with_nested_delegation(true)
    .resolve(&parent)
    .unwrap();
    assert!(inherited.get("required_tool").is_some());
    assert!(inherited.get("delegate").is_some());
    assert!(inherited.get("subagent_info").is_some());

    let denied_result =
        SubagentToolInheritancePolicy::new(vec!["required_tool".to_string()], vec![])
            .with_denied_tools(vec!["required_tool".to_string()])
            .resolve(&parent);
    assert!(denied_result.is_err());
    let denied = denied_result.err().unwrap();
    assert_eq!(
        denied.to_string(),
        "required inherited tool is denied: required_tool"
    );

    let missing_result = SubagentToolInheritancePolicy::new(vec!["absent".to_string()], vec![])
        .without_auto_inherit()
        .resolve(&parent);
    assert!(missing_result.is_err());
    let missing = missing_result.err().unwrap();
    assert_eq!(
        missing.to_string(),
        "required inherited tool is missing: absent"
    );
}

#[tokio::test]
async fn subagent_registry_insert_names_availability_missing_and_recursion_paths() {
    let child = Arc::new(AgentBuilder::new(Arc::new(TestModel::with_text("child"))).build());
    let mut registry = SubagentRegistry::new();
    assert!(registry.is_empty());
    registry.insert(SubagentConfig::new("child", child));
    assert_eq!(registry.names(), vec!["child".to_string()]);
    assert!(registry.is_available("child"));

    let mut context = AgentContext::default();
    let missing = registry
        .delegate("missing", "hello", &mut context)
        .await
        .unwrap_err();
    assert!(missing.to_string().contains("missing subagent missing"));
    assert_eq!(context.events.events()[0].kind, "subagent_failed");

    let mut recursive_context = AgentContext::default();
    recursive_context.metadata.insert(
        "starweaver.subagent_stack".to_string(),
        serde_json::json!(["child"]),
    );
    let recursive = registry
        .delegate_task(
            "child",
            starweaver_agent::SubagentTask::new("hello")
                .with_id(TaskId::from_string("task-recursive")),
            &mut recursive_context,
        )
        .await
        .unwrap_err();
    assert!(recursive
        .to_string()
        .contains("recursive subagent delegation for child"));
    assert!(recursive_context
        .events
        .events()
        .iter()
        .any(|event| event.kind == "subagent_failed"));
}

#[tokio::test]
async fn delegate_tool_carries_metadata_agent_id_and_parent_tools() {
    let child_model = TestModel::with_responses(vec![
        starweaver_model::tool_call_response("call_1", "inherited_tool", serde_json::json!({})),
        ModelResponse {
            usage: Usage {
                requests: 1,
                tool_calls: 0,
                ..Usage::default()
            },
            ..ModelResponse::text("child done")
        },
    ]);
    let inherited_tool = Arc::new(FunctionTool::new(
        "inherited_tool",
        Some("Inherited tool".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, _args: serde_json::Value| async move {
            Ok(ToolResult::new(serde_json::json!({"ok": true})))
        },
    ));
    let child = Arc::new(
        AgentBuilder::new(Arc::new(child_model))
            .tool(inherited_tool.clone())
            .build(),
    );
    let registry = Arc::new(SubagentRegistry::new().with_subagent(
        SubagentConfig::new("child", child).with_tool_inheritance(
            SubagentToolInheritancePolicy::new(vec!["inherited_tool".to_string()], vec![]),
        ),
    ));
    let tool = registry.delegate_tool_named("delegate_child");
    let parent_context = AgentContext::default();
    let handle = starweaver_context::AgentContextHandle::new(parent_context);
    let parent_tools = SubagentParentTools(ToolRegistry::new().with_tool(inherited_tool));
    let mut tool_context = ToolContext::new(RunId::new(), ConversationId::new(), 0);
    tool_context.dependencies.insert(handle.clone());
    tool_context.dependencies.insert(parent_tools);

    let result = tool
        .call(
            tool_context,
            serde_json::json!({
                "subagent_name": "child",
                "prompt": "use inherited tool",
                "agent_id": "agent-child",
                "metadata": {"source": "tool"}
            }),
        )
        .await
        .unwrap();

    assert_eq!(result.content["output"], "child done");
    assert_eq!(result.metadata["context_mutated"], true);
    let snapshot = handle.snapshot();
    assert!(snapshot
        .events
        .events()
        .iter()
        .any(|event| event.payload["metadata"]["agent_id"] == "agent-child"));
    assert!(snapshot.usage.requests >= 1);
}
