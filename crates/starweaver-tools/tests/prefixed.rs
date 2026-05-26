#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::{Arc, Mutex};

use starweaver_core::{ConversationId, RunId};
use starweaver_tools::{
    DynToolset, FunctionTool, PrefixedTool, PrefixedToolset, StaticToolset, Tool, ToolContext,
    ToolInstruction, ToolRegistry, ToolResult,
};

#[tokio::test]
async fn prefixed_tool_exposes_prefixed_name_and_delegates_execution() {
    let called = Arc::new(Mutex::new(false));
    let called_clone = called.clone();
    let inner = FunctionTool::new(
        "lookup",
        Some("Lookup".to_string()),
        serde_json::json!({"type": "object"}),
        move |_ctx: ToolContext, args: serde_json::Value| {
            let called = called_clone.clone();
            async move {
                *called.lock().unwrap() = true;
                Ok(ToolResult::new(serde_json::json!({"value": args["query"]})))
            }
        },
    );
    let prefixed = PrefixedTool::new("weather", Arc::new(inner));

    assert_eq!(prefixed.name(), "weather_lookup");
    assert_eq!(prefixed.description(), Some("Lookup"));
    let result = prefixed
        .call(
            ToolContext::new(RunId::new(), ConversationId::new(), 0),
            serde_json::json!({"query": "Paris"}),
        )
        .await
        .unwrap();

    assert!(*called.lock().unwrap());
    assert_eq!(result.content["value"], "Paris");
}

#[tokio::test]
async fn prefixed_toolset_prefixes_tools_and_instruction_groups() {
    let inner_tool = FunctionTool::new(
        "conditions",
        Some("Conditions".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, args| async move { Ok(ToolResult::new(args)) },
    );
    let toolset: DynToolset = Arc::new(
        StaticToolset::new("weather")
            .with_tool(Arc::new(inner_tool))
            .with_instruction(ToolInstruction::new("weather", "Use weather tools.")),
    );
    let prefixed: DynToolset = Arc::new(PrefixedToolset::new("api", toolset));
    let registry = ToolRegistry::new().with_toolset(&prefixed);

    let definitions = registry.definitions();
    assert_eq!(definitions.len(), 1);
    assert_eq!(definitions[0].name, "api_conditions");
    assert_eq!(registry.instructions(), vec!["Use weather tools."]);
    let call = starweaver_model::ToolCallPart {
        id: "call_1".to_string(),
        name: "api_conditions".to_string(),
        arguments: serde_json::json!({"city": "Paris"}),
    };
    let result = registry
        .execute_call(
            ToolContext::new(RunId::new(), ConversationId::new(), 0),
            &call,
        )
        .await;

    assert_eq!(result.name, "api_conditions");
    assert_eq!(result.content["city"], "Paris");
    assert!(!result.is_error);
}
