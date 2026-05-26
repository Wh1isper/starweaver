#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use starweaver_model::{FunctionModel, ModelResponse, ToolDefinition};
use starweaver_runtime::{Agent, AgentCapability, CapabilityResult};
use starweaver_tools::{DynTool, FunctionTool, ToolContext, ToolRegistry, ToolResult};

struct FilterToolsCapability {
    seen: Arc<Mutex<Vec<Vec<String>>>>,
}

#[async_trait]
impl AgentCapability for FilterToolsCapability {
    async fn prepare_tools(
        &self,
        _state: &starweaver_runtime::AgentRunState,
        tools: Vec<ToolDefinition>,
    ) -> CapabilityResult<Vec<ToolDefinition>> {
        self.seen
            .lock()
            .unwrap()
            .push(tools.iter().map(|tool| tool.name.clone()).collect());
        Ok(tools
            .into_iter()
            .filter(|tool| tool.name != "blocked")
            .collect())
    }
}

fn tool(name: &'static str) -> DynTool {
    Arc::new(FunctionTool::new(
        name,
        Some(format!("{name} tool")),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, args| std::future::ready(Ok(ToolResult::new(args))),
    ))
}

#[tokio::test]
async fn capability_can_prepare_tool_definitions_before_model_request() {
    let seen = Arc::new(Mutex::new(Vec::<Vec<String>>::new()));
    let model = FunctionModel::new(|_messages, _settings, info| {
        let tool_names = info
            .params
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(tool_names, vec!["allowed"]);
        Ok(ModelResponse::text("ok"))
    });
    let tools = ToolRegistry::new()
        .with_tool(tool("allowed"))
        .with_tool(tool("blocked"));

    let result = Agent::new(Arc::new(model))
        .with_tools(tools)
        .with_capability(Arc::new(FilterToolsCapability { seen: seen.clone() }))
        .run("hello")
        .await
        .unwrap();

    assert_eq!(result.output, "ok");
    assert_eq!(seen.lock().unwrap()[0], vec!["allowed", "blocked"]);
}
