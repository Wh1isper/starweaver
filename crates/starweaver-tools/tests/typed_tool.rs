#![allow(missing_docs, clippy::unwrap_used)]

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use starweaver_core::{ConversationId, RunId};
use starweaver_tools::{typed_tool, Tool, ToolContext, ToolResult};

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct AddArgs {
    /// Left integer to add.
    left: i64,
    #[schemars(description = "Right integer to add.")]
    right: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct BatchArgs {
    /// Batch entries to process.
    entries: Vec<BatchEntry>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct BatchEntry {
    /// Entry key.
    key: String,
    /// Entry value.
    value: String,
}

#[tokio::test]
async fn typed_tool_derives_schema_and_validates_arguments() {
    let tool = typed_tool::<AddArgs, _, _>(
        "add",
        Some("Add two integers.".to_string()),
        |_context: ToolContext, arguments: AddArgs| async move {
            Ok(ToolResult::new(serde_json::json!({
                "sum": arguments.left + arguments.right,
            })))
        },
    );

    let schema = tool.parameters_schema();
    assert_eq!(schema.get("$schema"), None);
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["properties"]["left"]["type"], "integer");
    assert_eq!(
        schema["properties"]["left"]["description"],
        "Left integer to add."
    );
    assert_eq!(schema["properties"]["right"]["type"], "integer");
    assert_eq!(
        schema["properties"]["right"]["description"],
        "Right integer to add."
    );
    assert!(schema["required"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("left")));

    let result = tool
        .call(
            ToolContext::new(RunId::new(), ConversationId::new(), 0),
            serde_json::json!({"left": 2, "right": 3}),
        )
        .await
        .unwrap();
    assert_eq!(result.content["sum"], 5);

    let error = tool
        .call(
            ToolContext::new(RunId::new(), ConversationId::new(), 0),
            serde_json::json!({"left": 2}),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("invalid tool arguments"));
}

#[tokio::test]
async fn typed_tool_preserves_nested_schema_descriptions_and_required_fields() {
    let tool = typed_tool::<BatchArgs, _, _>(
        "batch",
        Some("Process entries.".to_string()),
        |_context: ToolContext, arguments: BatchArgs| async move {
            Ok(ToolResult::new(json!({"count": arguments.entries.len()})))
        },
    );

    let schema = tool.parameters_schema();
    assert_eq!(schema.get("$schema"), None);
    assert_eq!(schema["type"], "object");
    assert!(schema["required"]
        .as_array()
        .unwrap()
        .contains(&json!("entries")));
    assert_eq!(
        schema["properties"]["entries"]["description"],
        "Batch entries to process."
    );
    assert_eq!(schema["$defs"]["BatchEntry"]["type"], "object");
    assert_eq!(
        schema["$defs"]["BatchEntry"]["properties"]["key"]["description"],
        "Entry key."
    );
    assert_eq!(
        schema["$defs"]["BatchEntry"]["properties"]["value"]["description"],
        "Entry value."
    );
    assert!(schema["$defs"]["BatchEntry"]["required"]
        .as_array()
        .unwrap()
        .contains(&json!("key")));

    let result = tool
        .call(
            ToolContext::new(RunId::new(), ConversationId::new(), 0),
            json!({"entries": [{"key": "a", "value": "b"}]}),
        )
        .await
        .unwrap();
    assert_eq!(result.content["count"], 1);
}
