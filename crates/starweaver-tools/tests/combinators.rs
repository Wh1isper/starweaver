#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use starweaver_core::{ConversationId, Metadata, RunId};
use starweaver_tools::{
    ApprovalRequiredToolset, DeferredLoadingToolset, DynTool, DynToolset, DynamicToolset,
    FilteredToolset, FunctionTool, PreparedToolset, RenamedToolset, StaticToolset,
    ToolApprovalState, ToolContext, ToolInstruction, ToolRegistry, ToolResult, Toolset,
};

fn echo_tool(name: &str) -> DynTool {
    Arc::new(FunctionTool::new(
        name,
        Some("Echo".to_string()),
        serde_json::json!({"type":"object"}),
        |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
    ))
}

#[tokio::test]
async fn filtered_toolset_limits_discovery_and_dispatch() {
    let inner = Arc::new(
        StaticToolset::new("tools")
            .with_tool(echo_tool("keep"))
            .with_tool(echo_tool("drop")),
    );
    let filtered: DynToolset = Arc::new(FilteredToolset::allow_names(inner, ["keep"]));
    let registry = ToolRegistry::new().with_toolset(&filtered);

    let definitions = registry.definitions();
    assert_eq!(definitions.len(), 1);
    assert_eq!(definitions[0].name, "keep");

    let result = registry
        .execute_call(
            ToolContext::new(RunId::new(), ConversationId::new(), 0),
            &starweaver_model::ToolCallPart {
                id: "call_keep".to_string(),
                name: "keep".to_string(),
                arguments: serde_json::json!({"ok": true}).into(),
            },
        )
        .await;
    assert_eq!(result.content["ok"], true);
}

#[tokio::test]
async fn renamed_toolset_exposes_alias_and_delegates_to_inner_tool() {
    let inner = Arc::new(StaticToolset::new("tools").with_tool(echo_tool("search")));
    let renamed: DynToolset = Arc::new(RenamedToolset::new(
        inner,
        [("search".to_string(), "web_search".to_string())],
    ));
    let registry = ToolRegistry::new().with_toolset(&renamed);

    let definitions = registry.definitions();
    assert_eq!(definitions[0].name, "web_search");
    assert_eq!(definitions[0].metadata["original_tool_name"], "search");

    let result = registry
        .execute_call(
            ToolContext::new(RunId::new(), ConversationId::new(), 0),
            &starweaver_model::ToolCallPart {
                id: "call_search".to_string(),
                name: "web_search".to_string(),
                arguments: serde_json::json!({"query": "rust"}).into(),
            },
        )
        .await;
    assert_eq!(result.content["query"], "rust");
}

#[tokio::test]
async fn approval_required_toolset_marks_blocks_and_honors_inline_approval() {
    let mut metadata = Metadata::default();
    metadata.insert("bundle".to_string(), serde_json::json!("filesystem"));
    let write_tool = FunctionTool::new(
        "write",
        Some("Write".to_string()),
        serde_json::json!({"type":"object"}),
        |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
    )
    .with_metadata(metadata);
    let inner = Arc::new(StaticToolset::new("core").with_tool(Arc::new(write_tool)));
    let gated: DynToolset = Arc::new(ApprovalRequiredToolset::new(inner, ["filesystem"]));
    let registry = ToolRegistry::new().with_toolset(&gated);

    let definitions = registry.definitions();
    assert_eq!(definitions[0].metadata["approval_required"], true);

    let result = registry
        .execute_call(
            ToolContext::new(RunId::new(), ConversationId::new(), 0),
            &starweaver_model::ToolCallPart {
                id: "call_write".to_string(),
                name: "write".to_string(),
                arguments: serde_json::json!({"path": "file.txt"}).into(),
            },
        )
        .await;
    assert!(result.is_error);
    assert_eq!(result.content["kind"], "approval_required");
    assert_eq!(result.metadata["approval"]["toolset"], "core");

    let approved = registry
        .execute_call(
            ToolContext::new(RunId::new(), ConversationId::new(), 0).with_approval(
                ToolApprovalState::Approved {
                    override_arguments: Some(serde_json::json!({"path": "approved.txt"})),
                    metadata: Metadata::default(),
                },
            ),
            &starweaver_model::ToolCallPart {
                id: "call_write_approved".to_string(),
                name: "write".to_string(),
                arguments: serde_json::json!({"path": "file.txt"}).into(),
            },
        )
        .await;
    assert!(!approved.is_error);
    assert_eq!(approved.content["path"], "approved.txt");
    assert_eq!(approved.metadata["approval_state"], "approved");
}

#[test]
fn prepared_dynamic_and_deferred_toolsets_materialize_tools() {
    let inner = Arc::new(
        StaticToolset::new("tools")
            .with_tool(echo_tool("first"))
            .with_tool(echo_tool("second"))
            .with_instruction(ToolInstruction::new("tools", "Use tools.")),
    );
    let prepared = PreparedToolset::new(inner.clone(), |tools| {
        tools
            .into_iter()
            .filter(|tool| tool.name() == "second")
            .collect()
    })
    .with_instruction_prepare(|mut instructions| {
        instructions.push(ToolInstruction::new("extra", "Prepared."));
        instructions
    });
    assert_eq!(prepared.get_tools()[0].name(), "second");
    assert_eq!(prepared.get_instructions().len(), 2);

    let dynamic = DynamicToolset::new("dynamic", || vec![echo_tool("dynamic_tool")])
        .with_instructions(|| vec![ToolInstruction::new("dynamic", "Use dynamic.")]);
    assert_eq!(dynamic.get_tools()[0].name(), "dynamic_tool");
    assert_eq!(dynamic.get_instructions()[0].content, "Use dynamic.");

    let loads = Arc::new(AtomicUsize::new(0));
    let loads_for_loader = loads.clone();
    let deferred = DeferredLoadingToolset::new("deferred", move || {
        loads_for_loader.fetch_add(1, Ordering::SeqCst);
        inner.clone()
    });
    assert_eq!(loads.load(Ordering::SeqCst), 0);
    assert_eq!(deferred.get_tools().len(), 2);
    assert_eq!(deferred.get_tools().len(), 2);
    assert_eq!(loads.load(Ordering::SeqCst), 1);
}
