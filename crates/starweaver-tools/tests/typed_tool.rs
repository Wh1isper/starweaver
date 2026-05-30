#![allow(missing_docs, clippy::unwrap_used)]

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use starweaver_core::{ConversationId, RunId};
use starweaver_tools::{typed_tool, Tool, ToolContext, ToolResult};

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct AddArgs {
    left: i64,
    right: i64,
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
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["properties"]["left"]["type"], "integer");
    assert_eq!(schema["properties"]["right"]["type"], "integer");
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
