use serde_json::{Map, Value};
use starweaver_context::AgentContextHandle;
use starweaver_tools::{ToolContext, ToolError, ToolResult};

use super::args::{NoteGetArgs, NoteSetArgs, SummarizeArgs, ThinkingArgs};

pub(super) async fn summarize(
    context: ToolContext,
    arguments: SummarizeArgs,
) -> Result<ToolResult, ToolError> {
    let Some(handle) = context.dependency::<AgentContextHandle>() else {
        return Err(ToolError::Execution {
            tool: "summarize".to_string(),
            message: "summarize requires AgentContextHandle".to_string(),
        });
    };
    let auto_load_files = arguments.auto_load_files.unwrap_or_default();
    let rendered = render_handoff_message(&arguments.content);
    handle.update(|agent_context| {
        agent_context.handoff_message = Some(rendered.clone());
        for file in &auto_load_files {
            if !agent_context.auto_load_files.contains(file) {
                agent_context.auto_load_files.push(file.clone());
            }
        }
    });

    Ok(operation(
        "summarize",
        serde_json::json!({
            "content": arguments.content,
            "rendered": rendered,
            "auto_load_files": auto_load_files,
        }),
    ))
}

fn render_handoff_message(content: &str) -> String {
    format!("# Context Summary\n\n{content}")
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
