#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use starweaver_agent::{
    AgentBuilder, AgentSpec, AgentSpecError, AgentSpecRegistry, FunctionTool, StaticToolset,
    SubagentConfig, TestModel, ToolContext, ToolError, ToolResult,
};
use starweaver_model::{
    ModelAdapter, ModelError, ModelMessage, ModelProfile, ModelRequestContext,
    ModelRequestParameters, ModelResponse, ModelSettings, ProtocolFamily,
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
        static PROFILE: ModelProfile =
            ModelProfile::for_protocol(ProtocolFamily::OpenAiChatCompletions);
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

    assert_eq!(captured_tools.lock().unwrap()[0], vec!["view"]);
    assert_eq!(app.subagents().names(), vec!["child"]);
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
        vec!["shell_exec", "view"]
    );
    assert_eq!(app.subagents().names(), vec!["child"]);
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
