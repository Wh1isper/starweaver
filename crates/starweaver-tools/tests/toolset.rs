#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use starweaver_tools::{
    DynToolset, FunctionTool, StaticToolset, ToolContext, ToolInstruction, ToolRegistry, ToolResult,
};

#[test]
fn registry_collects_toolsets_and_deduplicates_instructions() {
    let first = FunctionTool::new(
        "first",
        Some("First tool".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
    );
    let second = FunctionTool::new(
        "second",
        Some("Second tool".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
    );
    let toolset = StaticToolset::new("example")
        .with_tool(Arc::new(first))
        .with_tool(Arc::new(second))
        .with_instruction(ToolInstruction::new("example", "Use example tools."))
        .with_instruction(ToolInstruction::new("example", "Duplicate ignored."));

    let toolset: DynToolset = Arc::new(toolset);
    let registry = ToolRegistry::new().with_toolset(&toolset);

    assert_eq!(registry.definitions().len(), 2);
    assert_eq!(
        registry.instructions(),
        vec!["Use example tools.".to_string()]
    );
}
