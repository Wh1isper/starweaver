#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use serde_json::json;
use starweaver_core::{ConversationId, RunId};
use starweaver_model::ToolCallPart;
use starweaver_tools::{
    string_tool, DynTool, DynToolset, FunctionTool, StaticToolset, ToolContext, ToolError,
    ToolInstruction, ToolRegistry, ToolResult, Toolset,
};

fn context() -> ToolContext {
    ToolContext::new(RunId::from_string("run_registry"), ConversationId::new(), 0)
}

#[tokio::test]
async fn registry_dispatch_selects_removes_and_auto_inherits_tools() {
    let mut metadata = serde_json::Map::new();
    metadata.insert("auto_inherit".to_string(), json!(true));
    let inherited = FunctionTool::new(
        "inherited",
        Some("Auto inherited tool".to_string()),
        json!({"type":"object"}),
        |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
    )
    .with_metadata(metadata)
    .with_max_retries(4);
    let failing = FunctionTool::new(
        "failing",
        Some("Failing tool".to_string()),
        json!({"type":"object"}),
        |_ctx: ToolContext, _args: serde_json::Value| async move {
            Err(ToolError::ModelRetry {
                tool: "failing".to_string(),
                message: "retry please".to_string(),
            })
        },
    );
    let tools: Vec<DynTool> = vec![Arc::new(inherited), Arc::new(failing)];
    let toolset = StaticToolset::new("registry_extra")
        .with_id("registry_extra")
        .with_max_retries(2)
        .with_tools(tools)
        .with_instructions(vec![
            ToolInstruction::new("a", "Use a."),
            ToolInstruction::new("b", "Use b."),
        ]);
    assert_eq!(toolset.id(), Some("registry_extra"));
    assert_eq!(toolset.max_retries(), Some(2));
    let toolset: DynToolset = Arc::new(toolset);
    let mut registry = ToolRegistry::new()
        .with_max_retries(7)
        .with_toolset(&toolset);
    assert_eq!(registry.max_retries(), Some(7));
    assert!(!registry.is_empty());
    assert_eq!(registry.names(), vec!["failing", "inherited"]);
    assert_eq!(registry.tools().len(), 2);
    assert_eq!(registry.definitions().len(), 2);
    assert_eq!(registry.max_retries_for("inherited"), 4);
    assert_eq!(registry.max_retries_for("failing"), 2);
    assert_eq!(registry.max_retries_for("missing"), 7);
    assert_eq!(registry.get_instructions(), vec!["Use a.", "Use b."]);

    let call = ToolCallPart {
        id: "call_1".to_string(),
        name: "inherited".to_string(),
        arguments: json!({"ok": true}),
    };
    let returned = registry.execute_call(context(), &call).await;
    assert!(!returned.is_error);
    assert_eq!(returned.content["ok"], true);

    let error_call = ToolCallPart {
        id: "call_2".to_string(),
        name: "failing".to_string(),
        arguments: json!({}),
    };
    let error = registry.execute_call(context(), &error_call).await;
    assert!(error.is_error);
    assert_eq!(error.metadata["error_kind"], "model_retry");

    let missing_call = ToolCallPart {
        id: "call_3".to_string(),
        name: "missing".to_string(),
        arguments: json!({}),
    };
    assert!(
        registry
            .execute_call(context(), &missing_call)
            .await
            .is_error
    );

    let inherited_only = registry.auto_inherited();
    assert!(inherited_only.contains("inherited"));
    assert!(!inherited_only.contains("failing"));
    assert!(inherited_only.get("inherited").is_some());

    let selected = registry.select(["failing"]);
    assert!(selected.contains("failing"));
    assert!(!selected.contains("inherited"));
    assert_eq!(selected.max_retries_for("failing"), 2);

    assert!(registry.remove("failing").is_some());
    assert!(!registry.contains("failing"));
}

#[test]
fn registry_insert_registry_carries_retry_and_instructions() {
    let tool = string_tool(
        "plain",
        Some("Plain tool".to_string()),
        json!({"type":"object"}),
        |_ctx, args| async move { Ok(ToolResult::new(args)) },
    );
    let source_toolset = StaticToolset::new("source")
        .with_max_retries(3)
        .with_tool(Arc::new(tool))
        .with_instruction(ToolInstruction::new("source", "Use source."));
    let source_toolset: DynToolset = Arc::new(source_toolset);
    let source = ToolRegistry::new()
        .with_max_retries(5)
        .with_toolset(&source_toolset);
    let mut target = ToolRegistry::new();
    target.set_max_retries(1);
    target.insert_registry(&source);
    assert_eq!(target.max_retries(), Some(5));
    assert_eq!(target.max_retries_for("plain"), 3);
    assert_eq!(target.get_instructions(), vec!["Use source."]);
}
