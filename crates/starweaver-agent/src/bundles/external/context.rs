use serde_json::{Map, Value};
use starweaver_tools::{ToolContext, ToolError, ToolResult};

use super::args::{NoteGetArgs, NoteSetArgs, SummarizeArgs, ThinkingArgs};

pub(super) async fn summarize(
    _context: ToolContext,
    arguments: SummarizeArgs,
) -> Result<ToolResult, ToolError> {
    Ok(operation(
        "summarize",
        serde_json::json!({
            "content": arguments.content,
            "auto_load_files": arguments.auto_load_files.unwrap_or_default(),
        }),
    ))
}

pub(super) async fn note_set(
    _context: ToolContext,
    arguments: NoteSetArgs,
) -> Result<ToolResult, ToolError> {
    Ok(operation(
        "note",
        serde_json::json!({"key": arguments.key, "value": arguments.value}),
    ))
}

pub(super) async fn note_get(
    _context: ToolContext,
    arguments: NoteGetArgs,
) -> Result<ToolResult, ToolError> {
    Ok(operation(
        "note_get",
        serde_json::json!({"key": arguments.key}),
    ))
}

pub(super) async fn thinking(
    _context: ToolContext,
    arguments: ThinkingArgs,
) -> Result<ToolResult, ToolError> {
    Ok(operation(
        "thinking",
        serde_json::json!({"thought": arguments.thought}),
    ))
}

fn operation(name: &str, payload: Value) -> ToolResult {
    let mut content = Map::new();
    content.insert("operation".to_string(), Value::String(name.to_string()));
    content.insert("payload".to_string(), payload);
    ToolResult::new(Value::Object(content))
}
