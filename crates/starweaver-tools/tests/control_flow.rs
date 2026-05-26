#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use starweaver_core::{ConversationId, RunId};
use starweaver_tools::{FunctionTool, ToolContext, ToolError, ToolRegistry};

#[tokio::test]
async fn approval_required_error_returns_structured_control_flow_metadata() {
    let tool = FunctionTool::new(
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
    let call = starweaver_model::ToolCallPart {
        id: "call_1".to_string(),
        name: "dangerous".to_string(),
        arguments: serde_json::json!({}),
    };

    let result = ToolRegistry::new()
        .with_tool(Arc::new(tool))
        .execute_call(
            ToolContext::new(RunId::new(), ConversationId::new(), 0),
            &call,
        )
        .await;

    assert!(result.is_error);
    assert_eq!(result.content["kind"], "approval_required");
    assert_eq!(result.metadata["control_flow"], "approval_required");
    assert_eq!(result.metadata["approval"]["reason"], "delete");
}

#[tokio::test]
async fn deferred_call_error_returns_structured_control_flow_metadata() {
    let tool = FunctionTool::new(
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
    let call = starweaver_model::ToolCallPart {
        id: "call_1".to_string(),
        name: "slow".to_string(),
        arguments: serde_json::json!({}),
    };

    let result = ToolRegistry::new()
        .with_tool(Arc::new(tool))
        .execute_call(
            ToolContext::new(RunId::new(), ConversationId::new(), 0),
            &call,
        )
        .await;

    assert!(result.is_error);
    assert_eq!(result.content["kind"], "call_deferred");
    assert_eq!(result.metadata["control_flow"], "call_deferred");
    assert_eq!(result.metadata["deferred"]["queue"], "durable");
}
