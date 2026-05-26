#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use async_trait::async_trait;
use starweaver_model::{FunctionModel, ModelResponse, ToolDefinition};
use starweaver_runtime::{Agent, AgentCapability, CapabilityResult};
use starweaver_tools::{FunctionTool, ToolContext, ToolRegistry, ToolResult};

struct MarkApprovalCapability;

#[async_trait]
impl AgentCapability for MarkApprovalCapability {
    async fn prepare_tools(
        &self,
        _state: &starweaver_runtime::AgentRunState,
        tools: Vec<ToolDefinition>,
    ) -> CapabilityResult<Vec<ToolDefinition>> {
        Ok(tools
            .into_iter()
            .map(|mut tool| {
                if tool.name == "dangerous" {
                    tool.metadata
                        .insert("requires_approval".to_string(), serde_json::json!(true));
                }
                tool
            })
            .collect())
    }
}

#[tokio::test]
async fn prepare_tools_can_annotate_tool_metadata() {
    let model = FunctionModel::new(|_messages, _settings, info| {
        let tool = info
            .params
            .tools
            .iter()
            .find(|tool| tool.name == "dangerous")
            .unwrap();
        assert_eq!(tool.metadata["requires_approval"], true);
        Ok(ModelResponse::text("ok"))
    });
    let tool = FunctionTool::new(
        "dangerous",
        Some("Dangerous operation".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, args| async move { Ok(ToolResult::new(args)) },
    );

    let result = Agent::new(Arc::new(model))
        .with_tools(ToolRegistry::new().with_tool(Arc::new(tool)))
        .with_capability(Arc::new(MarkApprovalCapability))
        .run("hello")
        .await
        .unwrap();

    assert_eq!(result.output, "ok");
}
