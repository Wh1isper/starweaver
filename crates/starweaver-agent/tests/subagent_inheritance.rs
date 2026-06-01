#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use starweaver_agent::{
    AgentBuilder, AgentContext, FunctionTool, SubagentConfig, SubagentRegistry,
    SubagentToolInheritanceError, SubagentToolInheritancePolicy, TestModel, ToolContext, ToolError,
    ToolRegistry, ToolResult,
};
use starweaver_core::Metadata;
use starweaver_model::{tool_call_response, ModelResponse};

type ReadyToolResult = std::future::Ready<Result<ToolResult, ToolError>>;
type ReadyFunctionTool = FunctionTool<fn(ToolContext, serde_json::Value) -> ReadyToolResult>;

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
    assert!(context
        .events
        .events()
        .iter()
        .any(|event| event.kind == "subagent_completed"));
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
