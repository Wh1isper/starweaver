#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::{Arc, LazyLock, Mutex};

use async_trait::async_trait;
use starweaver_agent::{
    attach_environment, filesystem_tools, AgentBuilder, AgentContext, AgentSpec, AgentSpecError,
    AgentSpecRegistry, AgentStreamEvent, DynToolset, FunctionModel, FunctionTool, PrefixedToolset,
    RunStatus, SkillPackage, SkillRegistry, StaticCapabilityBundle, StaticToolset, SubagentConfig,
    SubagentRegistry, SubagentToolInheritancePolicy, TestModel, ToolContext, ToolError, ToolResult,
    SKILL_SCAN_EVENT_KIND,
};
use starweaver_environment::VirtualEnvironmentProvider;
use starweaver_model::{
    ModelAdapter, ModelError, ModelMessage, ModelProfile, ModelRequestContext,
    ModelRequestParameters, ModelRequestPart, ModelResponse, ModelResponsePart, ModelSettings,
    ProtocolFamily, ToolCallPart,
};

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
        self.captured_tools.lock().unwrap().push(
            params
                .tools
                .iter()
                .map(|tool| tool.name.clone())
                .collect::<Vec<_>>(),
        );
        Ok(ModelResponse::text("ok"))
    }
}

type ReadyTool = FunctionTool<
    fn(ToolContext, serde_json::Value) -> std::future::Ready<Result<ToolResult, ToolError>>,
>;

fn ready_tool_call(
    _ctx: ToolContext,
    args: serde_json::Value,
) -> std::future::Ready<Result<ToolResult, ToolError>> {
    std::future::ready(Ok(ToolResult::new(args)))
}

fn tool(name: &'static str) -> Arc<ReadyTool> {
    Arc::new(FunctionTool::new(
        name,
        Some(format!("{name} tool")),
        serde_json::json!({"type": "object"}),
        ready_tool_call
            as fn(
                ToolContext,
                serde_json::Value,
            ) -> std::future::Ready<Result<ToolResult, ToolError>>,
    ))
}

#[tokio::test]
async fn agent_spec_selects_named_toolsets_and_subagents() {
    let captured_tools = Arc::new(Mutex::new(Vec::new()));
    let model = Arc::new(ToolCaptureModel {
        captured_tools: captured_tools.clone(),
    });
    let filesystem = Arc::new(
        StaticToolset::new("filesystem")
            .with_id("fs")
            .with_tool(tool("view")),
    );
    let shell = Arc::new(StaticToolset::new("shell").with_tool(tool("shell_exec")));
    let child = Arc::new(AgentBuilder::new(Arc::new(TestModel::with_text("child"))).build());
    let spec = AgentSpec::from_yaml(
        r"
name: selected
model:
  model_id: capture
toolsets:
  - fs
subagents:
  - child
",
    )
    .unwrap();
    let registry = AgentSpecRegistry::new()
        .with_model("capture", model)
        .with_toolset(filesystem)
        .with_toolset(shell)
        .with_subagent(SubagentConfig::new("child", child));

    let app = spec.builder(&registry).unwrap().build_app();
    app.run("hello").await.unwrap();

    assert_eq!(
        captured_tools.lock().unwrap()[0],
        vec!["delegate", "subagent_info", "view"]
    );
    assert_eq!(app.subagents().names(), vec!["child"]);
}

#[tokio::test]
async fn subagent_config_from_agent_spec_materializes_selected_toolsets() {
    let captured_tools = Arc::new(Mutex::new(Vec::new()));
    let model = Arc::new(ToolCaptureModel {
        captured_tools: captured_tools.clone(),
    });
    let filesystem = Arc::new(StaticToolset::new("filesystem").with_tool(tool("view")));
    let spec = AgentSpec::from_yaml(
        r"
name: child
description: Child helper
model:
  model_id: capture
toolsets:
  - filesystem
",
    )
    .unwrap();
    let registry = AgentSpecRegistry::new()
        .with_model("capture", model)
        .with_toolset(filesystem);
    let config =
        SubagentConfig::from_agent_spec(&spec, &registry, SubagentToolInheritancePolicy::default())
            .unwrap();
    let subagents = SubagentRegistry::new().with_subagent(config);
    let mut context = AgentContext::default();

    let result = subagents
        .delegate("child", "hello", &mut context)
        .await
        .unwrap();

    assert_eq!(result.output, "ok");
    assert_eq!(captured_tools.lock().unwrap()[0], vec!["view"]);
}

#[tokio::test]
async fn subagent_config_from_agent_spec_applies_child_environment_provider() {
    let child_model = Arc::new(FunctionModel::new(|messages, _settings, _info| {
        let tool_return_text = messages
            .iter()
            .filter_map(|message| match message {
                ModelMessage::Request(request) => Some(&request.parts),
                ModelMessage::Response(_) => None,
            })
            .flatten()
            .find_map(|part| match part {
                ModelRequestPart::ToolReturn(tool_return) => Some(tool_return.content.to_string()),
                _ => None,
            })
            .unwrap_or_default();
        if tool_return_text.contains("child file") {
            Ok(ModelResponse::text("child environment"))
        } else if tool_return_text.contains("parent file") {
            Ok(ModelResponse::text("parent environment"))
        } else {
            Ok(ModelResponse {
                parts: vec![ModelResponsePart::ToolCall(ToolCallPart {
                    id: "view-readme".to_string(),
                    name: "view".to_string(),
                    arguments: serde_json::json!({"path": "README.md"}).into(),
                })],
                ..ModelResponse::text("")
            })
        }
    }));
    let spec = AgentSpec::from_yaml(
        r"
name: env-child
description: Environment-owned child
model:
  model_id: child-model
toolsets:
  - filesystem
preset:
  environment:
    provider: child-env
",
    )
    .unwrap();
    let registry = AgentSpecRegistry::new()
        .with_model("child-model", child_model)
        .with_toolset(filesystem_tools())
        .with_environment_provider(
            "child-env",
            Arc::new(
                VirtualEnvironmentProvider::new("child-env").with_file("README.md", "child file\n"),
            ),
        );
    let config =
        SubagentConfig::from_agent_spec(&spec, &registry, SubagentToolInheritancePolicy::default())
            .unwrap();
    assert_eq!(
        config
            .environment_provider()
            .as_ref()
            .map(|provider| provider.id()),
        Some("child-env")
    );
    let subagents = SubagentRegistry::new().with_subagent(config);
    let mut parent_context = AgentContext::default();
    attach_environment(
        &mut parent_context,
        Arc::new(
            VirtualEnvironmentProvider::new("parent-env").with_file("README.md", "parent file\n"),
        ),
    );

    let result = subagents
        .delegate("env-child", "read child workspace", &mut parent_context)
        .await
        .unwrap();

    assert_eq!(result.output, "child environment");
}

#[tokio::test]
async fn agent_spec_defaults_to_no_host_toolsets_or_subagents() {
    let captured_tools = Arc::new(Mutex::new(Vec::new()));
    let model = Arc::new(ToolCaptureModel {
        captured_tools: captured_tools.clone(),
    });
    let filesystem = Arc::new(StaticToolset::new("filesystem").with_tool(tool("view")));
    let child = Arc::new(AgentBuilder::new(Arc::new(TestModel::with_text("child"))).build());
    let spec = AgentSpec::from_yaml(
        r"
name: least-privilege
model:
  model_id: capture
",
    )
    .unwrap();
    let registry = AgentSpecRegistry::new()
        .with_model("capture", model)
        .with_toolset(filesystem)
        .with_subagent(SubagentConfig::new("child", child));

    let app = spec.builder(&registry).unwrap().build_app();
    app.run("hello").await.unwrap();

    assert!(captured_tools.lock().unwrap()[0].is_empty());
    assert!(app.subagents().is_empty());
}

#[tokio::test]
async fn agent_spec_can_explicitly_attach_all_registered_toolsets_and_subagents() {
    let captured_tools = Arc::new(Mutex::new(Vec::new()));
    let model = Arc::new(ToolCaptureModel {
        captured_tools: captured_tools.clone(),
    });
    let filesystem = Arc::new(StaticToolset::new("filesystem").with_tool(tool("view")));
    let shell = Arc::new(StaticToolset::new("shell").with_tool(tool("shell_exec")));
    let child = Arc::new(AgentBuilder::new(Arc::new(TestModel::with_text("child"))).build());
    let spec = AgentSpec::from_yaml(
        r"
name: all-selected
model:
  model_id: capture
all_toolsets: true
all_subagents: true
",
    )
    .unwrap();
    let registry = AgentSpecRegistry::new()
        .with_model("capture", model)
        .with_toolset(filesystem)
        .with_toolset(shell)
        .with_subagent(SubagentConfig::new("child", child));

    let app = spec.builder(&registry).unwrap().build_app();
    app.run("hello").await.unwrap();

    assert_eq!(
        captured_tools.lock().unwrap()[0],
        vec!["delegate", "shell_exec", "subagent_info", "view"]
    );
    assert_eq!(app.subagents().names(), vec!["child"]);
}

#[tokio::test]
async fn agent_spec_materializes_approval_preset_and_toolset_wrapper() {
    let model = Arc::new(FunctionModel::new(|_messages, _settings, _info| {
        Ok(ModelResponse {
            parts: vec![ModelResponsePart::ToolCall(ToolCallPart {
                id: "call_dangerous".to_string(),
                name: "dangerous".to_string(),
                arguments: serde_json::json!({"path": "target/file.txt"}).into(),
            })],
            ..ModelResponse::text("")
        })
    }));
    let dangerous: DynToolset = Arc::new(
        StaticToolset::new("dangerous-tools")
            .with_id("danger")
            .with_tool(tool("dangerous")),
    );
    let preset_spec = AgentSpec::from_yaml(
        r"
name: approval-preset
model:
  model_id: approval
toolsets:
  - danger
preset:
  approval:
    approval_required_tools: [dangerous]
",
    )
    .unwrap();
    let wrapper_spec = AgentSpec::from_yaml(
        r"
name: approval-wrapper
model:
  model_id: approval
toolset_wrappers:
  - kind: approval_required
    toolset: danger
    params:
      tools: [dangerous]
",
    )
    .unwrap();
    let registry = AgentSpecRegistry::new()
        .with_model("approval", model)
        .with_toolset(dangerous);

    let mut preset_session = preset_spec
        .builder(&registry)
        .unwrap()
        .build_app()
        .session();
    let preset_waiting = preset_session.run("try it").await.unwrap();
    assert_eq!(preset_waiting.state.status, RunStatus::Waiting);

    let mut wrapper_session = wrapper_spec
        .builder(&registry)
        .unwrap()
        .build_app()
        .session();
    let wrapper_waiting = wrapper_session.run("try it").await.unwrap();
    assert_eq!(wrapper_waiting.state.status, RunStatus::Waiting);
}

#[tokio::test]
async fn agent_spec_materializes_deferred_toolset_wrapper() {
    let model = Arc::new(FunctionModel::new(|_messages, _settings, _info| {
        Ok(ModelResponse {
            parts: vec![ModelResponsePart::ToolCall(ToolCallPart {
                id: "call_slow".to_string(),
                name: "slow_work".to_string(),
                arguments: serde_json::json!({"job": "index"}).into(),
            })],
            ..ModelResponse::text("")
        })
    }));
    let slow_tools: DynToolset = Arc::new(
        StaticToolset::new("slow-tools")
            .with_id("slow")
            .with_tool(tool("slow_work")),
    );
    let spec = AgentSpec::from_yaml(
        r"
name: deferred-wrapper
model:
  model_id: deferred
toolset_wrappers:
  - kind: deferred
    toolset: slow
    params:
      deferred_tools: [slow_work]
",
    )
    .unwrap();
    let registry = AgentSpecRegistry::new()
        .with_model("deferred", model)
        .with_toolset(slow_tools);

    let mut session = spec.builder(&registry).unwrap().build_app().session();
    let waiting = session.run("try it").await.unwrap();

    assert_eq!(waiting.state.status, RunStatus::Waiting);
    assert_eq!(waiting.state.deferred_tool_returns.len(), 1);
    assert_eq!(
        waiting.state.deferred_tool_returns[0].metadata["control_flow"],
        "call_deferred"
    );
}

#[tokio::test]
async fn agent_spec_materializes_dynamic_filter_and_rename_wrappers() {
    let captured_tools = Arc::new(Mutex::new(Vec::new()));
    let model = Arc::new(ToolCaptureModel {
        captured_tools: captured_tools.clone(),
    });
    let docs = Arc::new(
        StaticToolset::new("docs")
            .with_id("docs_ns")
            .with_tool(tool("lookup_docs")),
    );
    let mixed = Arc::new(
        StaticToolset::new("mixed")
            .with_id("mixed_ns")
            .with_tool(tool("keep"))
            .with_tool(tool("drop")),
    );
    let dynamic_spec = AgentSpec::from_yaml(
        r"
name: dynamic-wrapper
model:
  model_id: capture
toolset_wrappers:
  - kind: dynamic
    toolset: docs_ns
    params:
      prefix: docs
",
    )
    .unwrap();
    let filtered_spec = AgentSpec::from_yaml(
        r"
name: filtered-wrapper
model:
  model_id: capture
toolset_wrappers:
  - kind: filtered
    toolset: mixed_ns
    params:
      include_tools: [keep]
",
    )
    .unwrap();
    let renamed_spec = AgentSpec::from_yaml(
        r"
name: renamed-wrapper
model:
  model_id: capture
toolset_wrappers:
  - kind: renamed
    toolset: mixed_ns
    params:
      mappings:
        keep: use_keep
",
    )
    .unwrap();
    let registry = AgentSpecRegistry::new()
        .with_model("capture", model)
        .with_toolset(docs)
        .with_toolset(mixed);

    dynamic_spec
        .builder(&registry)
        .unwrap()
        .build_app()
        .run("hello")
        .await
        .unwrap();
    filtered_spec
        .builder(&registry)
        .unwrap()
        .build_app()
        .run("hello")
        .await
        .unwrap();
    renamed_spec
        .builder(&registry)
        .unwrap()
        .build_app()
        .run("hello")
        .await
        .unwrap();

    let captured = captured_tools.lock().unwrap().clone();
    assert!(captured[0].contains(&"docs_search_tool".to_string()));
    assert!(captured[0].contains(&"docs_call_tool".to_string()));
    assert!(!captured[0].contains(&"lookup_docs".to_string()));
    assert_eq!(captured[1], vec!["keep".to_string()]);
    assert!(captured[2].contains(&"use_keep".to_string()));
    assert!(!captured[2].contains(&"keep".to_string()));
}

#[tokio::test]
async fn agent_spec_materializes_host_defined_toolset_wrapper() {
    let captured_tools = Arc::new(Mutex::new(Vec::new()));
    let model = Arc::new(ToolCaptureModel {
        captured_tools: captured_tools.clone(),
    });
    let docs = Arc::new(
        StaticToolset::new("docs")
            .with_id("docs_ns")
            .with_tool(tool("lookup_docs")),
    );
    let spec = AgentSpec::from_yaml(
        r"
name: host-wrapper
model:
  model_id: capture
toolset_wrappers:
  - kind: host_prefix
    toolset: docs_ns
    params:
      prefix: host
",
    )
    .unwrap();
    let registry = AgentSpecRegistry::new()
        .with_model("capture", model)
        .with_toolset(docs)
        .with_toolset_wrapper_factory("host_prefix", |wrapper, registry| {
            let prefix = wrapper
                .params
                .get("prefix")
                .and_then(serde_json::Value::as_str)
                .filter(|prefix| !prefix.trim().is_empty())
                .ok_or_else(|| AgentSpecError::InvalidToolsetWrapper {
                    kind: wrapper.kind.clone(),
                    reason: "missing prefix".to_string(),
                })?;
            let inner_key = wrapper.toolset.as_deref().ok_or_else(|| {
                AgentSpecError::InvalidToolsetWrapper {
                    kind: wrapper.kind.clone(),
                    reason: "missing toolset".to_string(),
                }
            })?;
            let inner = registry
                .resolve_toolset(inner_key)
                .ok_or_else(|| AgentSpecError::UnknownToolset(inner_key.to_string()))?;
            Ok(Arc::new(PrefixedToolset::new(prefix, inner)) as DynToolset)
        });

    spec.builder(&registry)
        .unwrap()
        .build_app()
        .run("hello")
        .await
        .unwrap();

    assert_eq!(captured_tools.lock().unwrap()[0], vec!["host_lookup_docs"]);
}

#[tokio::test]
async fn agent_spec_materializes_registered_skill_roots() {
    let mut skills = SkillRegistry::new();
    skills.insert(SkillPackage {
        name: "research".to_string(),
        description: "Gather sources".to_string(),
        path: "skills/research/SKILL.md".to_string(),
        body: None,
        metadata: serde_json::Map::new(),
    });
    let spec = AgentSpec::from_yaml(
        r"
name: skill-agent
model:
  model_id: test
skills:
  roots: [workspace]
",
    )
    .unwrap();
    let registry = AgentSpecRegistry::new()
        .with_model("test", Arc::new(TestModel::with_text("ok")))
        .with_skill_registry("workspace", skills);

    let stream = spec
        .builder(&registry)
        .unwrap()
        .build()
        .run_stream("hello")
        .await
        .unwrap();

    assert!(stream.events().iter().any(|record| {
        matches!(
            &record.event,
            AgentStreamEvent::Custom { event }
                if event.kind == SKILL_SCAN_EVENT_KIND
                    && event.payload["package_count"] == 1
                    && event.payload["packages"][0]["name"] == "research"
        )
    }));
}

#[tokio::test]
async fn agent_spec_materializes_registered_capability_bundles() {
    let captured_tools = Arc::new(Mutex::new(Vec::new()));
    let model = Arc::new(ToolCaptureModel {
        captured_tools: captured_tools.clone(),
    });
    let bundle = StaticCapabilityBundle::new("bundle").with_tool(tool("bundle_tool"));
    let spec = AgentSpec::from_yaml(
        r"
name: bundle-agent
model:
  model_id: capture
capability_refs:
  - bundle
",
    )
    .unwrap();
    let registry = AgentSpecRegistry::new()
        .with_model("capture", model)
        .with_capability_bundle("bundle", Arc::new(bundle));

    spec.builder(&registry)
        .unwrap()
        .build_app()
        .run("hello")
        .await
        .unwrap();

    assert_eq!(captured_tools.lock().unwrap()[0], vec!["bundle_tool"]);
}

#[test]
fn agent_spec_reports_unknown_selected_toolset_and_subagent() {
    let model = Arc::new(TestModel::with_text("ok"));
    let registry = AgentSpecRegistry::new().with_model("test", model);
    let missing_toolset = AgentSpec::from_yaml(
        r"
name: missing-toolset
model:
  model_id: test
toolsets:
  - missing
",
    )
    .unwrap();
    let missing_subagent = AgentSpec::from_yaml(
        r"
name: missing-subagent
model:
  model_id: test
subagents:
  - missing
",
    )
    .unwrap();
    let missing_toolset_with_all = AgentSpec::from_yaml(
        r"
name: missing-toolset-with-all
model:
  model_id: test
all_toolsets: true
toolsets:
  - missing
",
    )
    .unwrap();
    let missing_subagent_with_all = AgentSpec::from_yaml(
        r"
name: missing-subagent-with-all
model:
  model_id: test
all_subagents: true
subagents:
  - missing
",
    )
    .unwrap();

    let Err(toolset_error) = missing_toolset.builder(&registry) else {
        panic!("expected unknown toolset error");
    };
    let Err(subagent_error) = missing_subagent.builder(&registry) else {
        panic!("expected unknown subagent error");
    };
    let Err(toolset_with_all_error) = missing_toolset_with_all.builder(&registry) else {
        panic!("expected unknown toolset error with all_toolsets");
    };
    let Err(subagent_with_all_error) = missing_subagent_with_all.builder(&registry) else {
        panic!("expected unknown subagent error with all_subagents");
    };

    assert!(matches!(
        toolset_error,
        AgentSpecError::UnknownToolset(name) if name == "missing"
    ));
    assert!(matches!(
        subagent_error,
        AgentSpecError::UnknownSubagent(name) if name == "missing"
    ));
    assert!(matches!(
        toolset_with_all_error,
        AgentSpecError::UnknownToolset(name) if name == "missing"
    ));
    assert!(matches!(
        subagent_with_all_error,
        AgentSpecError::UnknownSubagent(name) if name == "missing"
    ));
}
