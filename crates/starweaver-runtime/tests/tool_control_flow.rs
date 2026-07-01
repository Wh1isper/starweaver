#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use starweaver_model::{
    ModelAdapter, ModelMessage, ModelRequestPart, ModelResponse, ModelResponsePart, TestModel,
    ToolCallPart,
};
use starweaver_runtime::{Agent, RunStatus};
use starweaver_tools::{FunctionTool, ToolContext, ToolError, ToolRegistry, ToolResult};

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

    let agent_model: Arc<dyn ModelAdapter> = model.clone();
    let result = Agent::new(agent_model)
        .with_tools(
            ToolRegistry::new()
                .with_tool(Arc::new(dangerous))
                .with_tool(Arc::new(slow)),
        )
        .run("run tools")
        .await
        .unwrap();

    assert_eq!(result.output, "");
    assert_eq!(result.state.status, RunStatus::Waiting);
    assert!(result.has_pending_hitl());
    assert!(result.state.has_pending_hitl());
    assert!(result.state.pending_tool_returns.is_empty());
    assert_eq!(model.captured_messages().len(), 1);
    assert_eq!(result.state.pending_approval_tool_returns.len(), 1);
    assert_eq!(result.state.deferred_tool_returns.len(), 1);
    assert_eq!(result.pending_approvals().len(), 1);
    assert_eq!(result.pending_deferred_tools().len(), 1);
    assert_eq!(result.state.pending_approvals().len(), 1);
    assert_eq!(result.state.pending_deferred_tools().len(), 1);
    assert_eq!(
        result
            .state
            .pending_hitl_tool_returns()
            .map(|tool_return| tool_return.name.as_str())
            .collect::<Vec<_>>(),
        vec!["dangerous", "slow"]
    );
    assert_eq!(
        result.state.pending_approval_tool_returns[0].metadata["control_flow"],
        "approval_required"
    );
    assert_eq!(
        result.state.deferred_tool_returns[0].metadata["control_flow"],
        "call_deferred"
    );
}

#[tokio::test]
async fn runtime_preserves_non_control_flow_tool_returns_when_hitl_waits() {
    let model = Arc::new(TestModel::with_responses(vec![
        ModelResponse {
            parts: vec![
                ModelResponsePart::ToolCall(ToolCallPart {
                    id: "normal".to_string(),
                    name: "normal".to_string(),
                    arguments: serde_json::json!({}).into(),
                }),
                ModelResponsePart::ToolCall(ToolCallPart {
                    id: "approval".to_string(),
                    name: "dangerous".to_string(),
                    arguments: serde_json::json!({}).into(),
                }),
            ],
            ..ModelResponse::text("")
        },
        ModelResponse::text("done"),
    ]));
    let normal = FunctionTool::new(
        "normal",
        Some("Normal operation".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, _args| async move { Ok(ToolResult::new(serde_json::json!({"ok": true}))) },
    );
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

    let agent_model: Arc<dyn ModelAdapter> = model.clone();
    let result = Agent::new(agent_model)
        .with_tools(
            ToolRegistry::new()
                .with_tool(Arc::new(normal))
                .with_tool(Arc::new(dangerous)),
        )
        .run("run tools")
        .await
        .unwrap();

    assert_eq!(result.state.status, RunStatus::Waiting);
    assert!(result.state.pending_tool_returns.is_empty());
    assert_eq!(model.captured_messages().len(), 1);
    let tool_return_ids = result
        .messages
        .iter()
        .filter_map(|message| match message {
            ModelMessage::Request(request) => Some(&request.parts),
            ModelMessage::Response(_) => None,
        })
        .flat_map(|parts| parts.iter())
        .filter_map(|part| match part {
            ModelRequestPart::ToolReturn(tool_return) => Some(tool_return.tool_call_id.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let normal_tool_call_ids = result
        .messages
        .iter()
        .filter_map(|message| match message {
            ModelMessage::Response(response) => Some(&response.parts),
            ModelMessage::Request(_) => None,
        })
        .flat_map(|parts| parts.iter())
        .filter_map(|part| match part {
            ModelResponsePart::ToolCall(call) if call.name == "normal" => Some(call.id.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(normal_tool_call_ids.len(), 1);
    assert!(normal_tool_call_ids[0].starts_with("sw-tool-"));
    assert_eq!(tool_return_ids, normal_tool_call_ids);
}
