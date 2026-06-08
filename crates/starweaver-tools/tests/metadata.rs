#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use serde_json::json;
use starweaver_tools::{FunctionTool, ToolContext, ToolRegistry, ToolResult};

#[test]
fn function_tool_metadata_is_exposed_on_tool_definition() {
    let mut metadata = serde_json::Map::new();
    metadata.insert("requires_approval".to_string(), serde_json::json!(true));
    metadata.insert("source".to_string(), serde_json::json!("test"));
    let tool = FunctionTool::new(
        "delete_record",
        Some("Delete a record".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, args| async move { Ok(ToolResult::new(args)) },
    )
    .with_metadata(metadata.clone());

    let definition = ToolRegistry::new()
        .with_tool(Arc::new(tool))
        .definitions()
        .remove(0);

    assert_eq!(definition.metadata, metadata);
}

#[tokio::test]
async fn tool_result_metadata_is_exposed_on_tool_return() {
    let tool = FunctionTool::new(
        "mutating_tool",
        Some("Return metadata".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, _args| async move {
            let mut result = ToolResult::new(serde_json::json!({"ok": true}));
            result
                .metadata
                .insert("context_mutated".to_string(), serde_json::json!(true));
            Ok(result)
        },
    );
    let registry = ToolRegistry::new().with_tool(Arc::new(tool));
    let result = registry
        .execute_call(
            ToolContext::new(
                starweaver_core::RunId::default(),
                starweaver_core::ConversationId::default(),
                0,
            ),
            &starweaver_model::ToolCallPart {
                id: "call-1".to_string(),
                name: "mutating_tool".to_string(),
                arguments: serde_json::json!({}).into(),
            },
        )
        .await;

    assert_eq!(result.content["ok"], true);
    assert_eq!(result.metadata["context_mutated"], true);
}

#[tokio::test]
async fn structured_tool_result_keeps_private_metadata_out_of_model_content() {
    let tool = FunctionTool::new(
        "structured_tool",
        Some("Return structured surfaces".to_string()),
        json!({"type": "object"}),
        |_ctx: ToolContext, _args| async move {
            let mut private_metadata = serde_json::Map::new();
            private_metadata.insert("secret_token".to_string(), json!("host-only"));
            Ok(ToolResult::new(json!({"raw": "application value"}))
                .with_model_content(json!({"summary": "model-safe"}))
                .with_user_content(json!({"markdown": "User-visible result"}))
                .with_app_value(json!({"domain": {"id": 42}}))
                .with_private_metadata(private_metadata))
        },
    );
    let registry = ToolRegistry::new().with_tool(Arc::new(tool));
    let result = registry
        .execute_call(
            ToolContext::new(
                starweaver_core::RunId::default(),
                starweaver_core::ConversationId::default(),
                0,
            ),
            &starweaver_model::ToolCallPart {
                id: "call-structured".to_string(),
                name: "structured_tool".to_string(),
                arguments: json!({}).into(),
            },
        )
        .await;

    assert_eq!(result.content, json!({"summary": "model-safe"}));
    assert_eq!(
        result.user_content,
        Some(json!({"markdown": "User-visible result"}))
    );
    assert_eq!(result.app_value, Some(json!({"domain": {"id": 42}})));
    assert_eq!(result.private_metadata["secret_token"], json!("host-only"));
    assert!(!result.content.to_string().contains("host-only"));
    assert!(!result.metadata.contains_key("secret_token"));
}
