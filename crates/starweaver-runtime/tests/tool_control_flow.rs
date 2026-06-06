#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use starweaver_model::{ModelResponse, ModelResponsePart, TestModel, ToolCallPart};
use starweaver_runtime::Agent;
use starweaver_tools::{FunctionTool, ToolContext, ToolError, ToolRegistry};

#[tokio::test]
async fn runtime_records_approval_and_deferred_tool_returns() {
    let model = Arc::new(TestModel::with_responses(vec![
        ModelResponse {
            parts: vec![
                ModelResponsePart::ToolCall(ToolCallPart {
                    id: "approval".to_string(),
                    name: "dangerous".to_string(),
                    arguments: serde_json::json!({}).into(),
                }),
                ModelResponsePart::ToolCall(ToolCallPart {
                    id: "deferred".to_string(),
                    name: "slow".to_string(),
                    arguments: serde_json::json!({}).into(),
                }),
            ],
            ..ModelResponse::text("")
        },
        ModelResponse::text("done"),
    ]));
    let dangerous = FunctionTool::new(
        "dangerous",
        Some("Dangerous operation".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, _args| async move {
            Err(ToolError::ApprovalRequired {
                tool: "dangerous".to_string(),
                metadata: serde_json::json!({"reason": "delete"}),
            })
        },
    );
    let slow = FunctionTool::new(
        "slow",
        Some("Slow operation".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, _args| async move {
            Err(ToolError::CallDeferred {
                tool: "slow".to_string(),
                metadata: serde_json::json!({"queue": "durable"}),
            })
        },
    );

    let result = Agent::new(model)
        .with_tools(
            ToolRegistry::new()
                .with_tool(Arc::new(dangerous))
                .with_tool(Arc::new(slow)),
        )
        .run("run tools")
        .await
        .unwrap();

    assert_eq!(result.output, "done");
    assert_eq!(result.state.pending_approval_tool_returns.len(), 1);
    assert_eq!(result.state.deferred_tool_returns.len(), 1);
    assert_eq!(
        result.state.pending_approval_tool_returns[0].metadata["control_flow"],
        "approval_required"
    );
    assert_eq!(
        result.state.deferred_tool_returns[0].metadata["control_flow"],
        "call_deferred"
    );
}
