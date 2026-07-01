#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use serde_json::json;
use starweaver_tools::{
    EmptyToolArgs, FunctionTool, TOOL_METADATA_HIDDEN_BY_TAGS_KEY, TOOL_METADATA_KIND_KEY,
    TOOL_METADATA_TAGS_KEY, Tool, ToolContext, ToolKind, ToolRegistry, ToolResult,
    TypedFunctionTool, set_tool_metadata_kind, tool_metadata_hidden_by_tags, tool_metadata_kind,
    tool_metadata_tags,
};

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

#[test]
fn function_tool_return_schema_is_exposed_on_tool_definition() {
    let return_schema = json!({
        "type": "object",
        "properties": {"ok": {"type": "boolean"}},
        "required": ["ok"]
    });
    let tool = FunctionTool::new(
        "status",
        Some("Return status".to_string()),
        json!({"type": "object"}),
        |_ctx: ToolContext, _args| async move { Ok(ToolResult::new(json!({"ok": true}))) },
    )
    .with_return_schema(return_schema.clone())
    .with_strict_schema(true)
    .with_sequential(true);

    let definition = ToolRegistry::new()
        .with_tool(Arc::new(tool))
        .definitions()
        .remove(0);

    assert_eq!(definition.return_schema, Some(return_schema));
    assert_eq!(definition.strict, Some(true));
    assert_eq!(definition.sequential, Some(true));
}

#[test]
fn function_tool_tags_are_normalized_on_tool_definition_metadata() {
    let mut metadata = serde_json::Map::new();
    metadata.insert(
        TOOL_METADATA_TAGS_KEY.to_string(),
        json!(["existing", "existing", "", 42]),
    );
    metadata.insert(
        TOOL_METADATA_HIDDEN_BY_TAGS_KEY.to_string(),
        json!("not-a-list"),
    );

    let tool = FunctionTool::new(
        "search_files",
        Some("Search files".to_string()),
        json!({"type": "object"}),
        |_ctx: ToolContext, args| async move { Ok(ToolResult::new(args)) },
    )
    .with_metadata(metadata)
    .with_tags([" read ", "read", "write", ""])
    .with_tag("workspace")
    .with_hidden_by_tags([" remote-search ", "remote-search", ""])
    .with_hidden_by_tag("remote-edit");

    let definition = tool.definition();

    assert_eq!(
        definition.metadata[TOOL_METADATA_TAGS_KEY],
        json!(["existing", "read", "write", "workspace"])
    );
    assert_eq!(
        definition.metadata[TOOL_METADATA_HIDDEN_BY_TAGS_KEY],
        json!(["remote-search", "remote-edit"])
    );
    assert_eq!(
        tool_metadata_tags(&definition.metadata),
        vec![
            "existing".to_string(),
            "read".to_string(),
            "write".to_string(),
            "workspace".to_string()
        ]
    );
    assert_eq!(
        tool_metadata_hidden_by_tags(&definition.metadata),
        vec!["remote-search".to_string(), "remote-edit".to_string()]
    );
}

#[test]
fn typed_function_tool_tags_are_exposed_on_tool_definition_metadata() {
    let tool = TypedFunctionTool::<EmptyToolArgs, _>::new(
        "summarize",
        Some("Summarize context".to_string()),
        |_ctx: ToolContext, _args: EmptyToolArgs| async move {
            Ok(ToolResult::new(json!({"ok": true})))
        },
    )
    .with_tag("context")
    .with_tags(["workspace", "context"])
    .with_hidden_by_tag("compact-summary");

    let definition = tool.definition();

    assert_eq!(
        definition.metadata[TOOL_METADATA_TAGS_KEY],
        json!(["context", "workspace"])
    );
    assert_eq!(
        definition.metadata[TOOL_METADATA_HIDDEN_BY_TAGS_KEY],
        json!(["compact-summary"])
    );
}

#[test]
fn tool_kind_taxonomy_is_exposed_on_tool_definition_metadata() {
    let tool = FunctionTool::new(
        "external_search",
        Some("Search externally".to_string()),
        json!({"type": "object"}),
        |_ctx: ToolContext, args| async move { Ok(ToolResult::new(args)) },
    )
    .with_kind(ToolKind::External);

    let definition = tool.definition();

    assert_eq!(
        definition.metadata[TOOL_METADATA_KIND_KEY],
        json!("external")
    );
    assert_eq!(
        tool_metadata_kind(&definition.metadata),
        Some(ToolKind::External)
    );

    let typed = TypedFunctionTool::<EmptyToolArgs, _>::new(
        "approval_gate",
        Some("Ask for approval".to_string()),
        |_ctx: ToolContext, _args: EmptyToolArgs| async move {
            Ok(ToolResult::new(json!({"ok": true})))
        },
    )
    .with_kind(ToolKind::Unapproved);

    assert_eq!(
        tool_metadata_kind(&typed.definition().metadata),
        Some(ToolKind::Unapproved)
    );

    let mut metadata = serde_json::Map::new();
    set_tool_metadata_kind(&mut metadata, ToolKind::Deferred);
    assert_eq!(tool_metadata_kind(&metadata), Some(ToolKind::Deferred));
    assert_eq!(ToolKind::Output.as_str(), "output");
    assert_eq!(
        ToolKind::from_metadata_value("function"),
        Some(ToolKind::Function)
    );
    assert_eq!(ToolKind::from_metadata_value("unknown"), None);
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
