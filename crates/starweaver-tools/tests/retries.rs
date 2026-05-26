//! Per-tool retry budget tests.

use std::sync::{Arc, Mutex};

use starweaver_core::{ConversationId, RunId};
use starweaver_tools::{
    FunctionTool, StaticToolset, Tool, ToolContext, ToolRegistry, ToolResult, Toolset,
};

#[test]
fn function_tool_definition_includes_retry_override() {
    let tool = FunctionTool::new(
        "search",
        Some("Search".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
    )
    .with_max_retries(3);

    let definition = tool.definition();

    assert_eq!(definition.metadata["max_retries"], serde_json::json!(3));
}

#[test]
fn retry_precedence_is_tool_then_toolset_then_registry_then_default() {
    let inherited = FunctionTool::new(
        "inherited",
        Some("Inherited".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
    );
    let overridden = FunctionTool::new(
        "overridden",
        Some("Overridden".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
    )
    .with_max_retries(5);
    let standalone = FunctionTool::new(
        "standalone",
        Some("Standalone".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
    );
    let toolset = StaticToolset::new("helpers")
        .with_max_retries(2)
        .with_tool(Arc::new(inherited))
        .with_tool(Arc::new(overridden));
    let toolset: Arc<dyn Toolset> = Arc::new(toolset);

    let registry = ToolRegistry::new()
        .with_max_retries(7)
        .with_toolset(&toolset)
        .with_tool(Arc::new(standalone));

    assert_eq!(registry.max_retries_for("overridden"), 5);
    assert_eq!(registry.max_retries_for("inherited"), 2);
    assert_eq!(registry.max_retries_for("standalone"), 7);
    assert_eq!(ToolRegistry::new().max_retries_for("missing"), 1);
}

#[tokio::test]
async fn retry_budget_is_visible_in_tool_context() {
    let observed = Arc::new(Mutex::new(None));
    let observed_clone = observed.clone();
    let tool = FunctionTool::new(
        "inspect",
        Some("Inspect retry state".to_string()),
        serde_json::json!({"type": "object"}),
        move |ctx: ToolContext, args: serde_json::Value| {
            let observed = observed_clone.clone();
            async move {
                if let Ok(mut observed) = observed.lock() {
                    *observed = Some((ctx.retry, ctx.max_retries, ctx.last_attempt()));
                }
                Ok(ToolResult::new(args))
            }
        },
    )
    .with_max_retries(4);
    let registry = ToolRegistry::new().with_tool(Arc::new(tool));
    let call = starweaver_model::ToolCallPart {
        id: "call_1".to_string(),
        name: "inspect".to_string(),
        arguments: serde_json::json!({}),
    };
    let context = ToolContext::new(RunId::new(), ConversationId::new(), 0).with_retry_budget(4, 4);

    let result = registry.execute_call(context, &call).await;

    assert!(!result.is_error);
    assert_eq!(
        observed.lock().map_or_else(|_| None, |observed| *observed),
        Some((4, 4, true))
    );
}
