#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

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
                arguments: serde_json::json!({}),
            },
        )
        .await;

    assert_eq!(result.content["ok"], true);
    assert_eq!(result.metadata["context_mutated"], true);
}
