#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use starweaver_core::{ConversationId, RunId};
use starweaver_tools::{
    DeferredToolSpec, DeferredToolset, DynToolset, FunctionTool, ToolContext, ToolError,
    ToolRegistry,
};

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
        arguments: serde_json::json!({}).into(),
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
    assert_eq!(
        result.metadata[starweaver_model::TOOL_RETURN_APPROVAL_ARGUMENTS_METADATA_KEY],
        serde_json::json!({})
    );
}

#[tokio::test]
async fn declarative_deferred_toolset_emits_request_and_accepts_resume_result() {
    let run_id = RunId::from_string("run-declared-deferred");
    let toolset: DynToolset = Arc::new(DeferredToolset::from_specs(
        "client-tools",
        [DeferredToolSpec::new(
            "client_lookup",
            "Look up a value in the client",
            serde_json::json!({"type": "object"}),
        )
        .with_instruction("Use the client for external lookups.")],
    ));
    let registry = ToolRegistry::new().with_toolset(&toolset);
    let call = starweaver_model::ToolCallPart {
        id: "call-client-lookup".to_string(),
        name: "client_lookup".to_string(),
        arguments: serde_json::json!({"query": "docs"}).into(),
    };

    let deferred = registry
        .execute_call(
            ToolContext::new(run_id.clone(), ConversationId::new(), 0),
            &call,
        )
        .await;
    assert!(deferred.is_error);
    assert_eq!(deferred.metadata["control_flow"], "call_deferred");
    assert_eq!(deferred.metadata["deferred"]["kind"], "client_tool_call");
    assert_eq!(
        deferred.metadata["deferred"]["deferred_id"],
        "deferred_run-declared-deferred_call-client-lookup"
    );
    assert_eq!(deferred.metadata["deferred"]["arguments"]["query"], "docs");

    let completed = registry
        .execute_call(
            ToolContext::new(run_id, ConversationId::new(), 1)
                .with_deferred_result(serde_json::json!({"answer": "found"})),
            &call,
        )
        .await;
    assert!(!completed.is_error);
    assert_eq!(completed.content["answer"], "found");
    assert_eq!(completed.metadata["deferred_state"], "completed");
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
        arguments: serde_json::json!({}).into(),
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
