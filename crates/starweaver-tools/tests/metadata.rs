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
