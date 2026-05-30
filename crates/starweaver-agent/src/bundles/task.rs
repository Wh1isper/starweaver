use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use starweaver_tools::{
    DynToolset, EmptyToolArgs, StaticToolset, ToolContext, ToolError, ToolInstruction, ToolResult,
};

use super::helpers::static_tool;

/// Create task operation tools.
#[must_use]
pub fn task_tools() -> DynToolset {
    Arc::new(
        StaticToolset::new("task")
            .with_id("task")
            .with_instruction(ToolInstruction::new(
                "task-manager",
                "Use task tools to create, inspect, list, and update lightweight task operation envelopes for the SDK host to persist or route.",
            ))
            .with_tools([
                static_tool("task_create", "Create a new task. Task status defaults to pending.", task_create),
                static_tool("task_get", "Get task details by ID.", task_get),
                static_tool("task_update", "Update task status, content, or dependencies.", task_update),
                static_tool("task_list", "List all tasks and their status.", task_list),
            ]),
    )
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct TaskCreateArgs {
    subject: String,
    description: String,
    active_form: Option<String>,
    metadata: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct TaskIdArgs {
    task_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct TaskUpdateArgs {
    task_id: String,
    status: Option<String>,
    subject: Option<String>,
    description: Option<String>,
    active_form: Option<String>,
    owner: Option<String>,
    add_blocks: Option<Vec<String>>,
    add_blocked_by: Option<Vec<String>>,
    metadata: Option<Value>,
}

async fn task_create(
    _context: ToolContext,
    arguments: TaskCreateArgs,
) -> Result<ToolResult, ToolError> {
    Ok(operation(
        "task_create",
        serde_json::json!({
            "subject": arguments.subject,
            "description": arguments.description,
            "active_form": arguments.active_form,
            "metadata": arguments.metadata.unwrap_or_else(|| serde_json::json!({})),
        }),
    ))
}

async fn task_get(_context: ToolContext, arguments: TaskIdArgs) -> Result<ToolResult, ToolError> {
    Ok(operation(
        "task_get",
        serde_json::json!({"task_id": arguments.task_id}),
    ))
}

async fn task_update(
    _context: ToolContext,
    arguments: TaskUpdateArgs,
) -> Result<ToolResult, ToolError> {
    Ok(operation(
        "task_update",
        serde_json::json!({
            "task_id": arguments.task_id,
            "status": arguments.status,
            "subject": arguments.subject,
            "description": arguments.description,
            "active_form": arguments.active_form,
            "owner": arguments.owner,
            "add_blocks": arguments.add_blocks.unwrap_or_default(),
            "add_blocked_by": arguments.add_blocked_by.unwrap_or_default(),
            "metadata": arguments.metadata.unwrap_or_else(|| serde_json::json!({})),
        }),
    ))
}

async fn task_list(
    _context: ToolContext,
    _arguments: EmptyToolArgs,
) -> Result<ToolResult, ToolError> {
    Ok(operation("task_list", serde_json::json!({})))
}

fn operation(name: &str, payload: Value) -> ToolResult {
    let mut content = serde_json::Map::new();
    content.insert("operation".to_string(), Value::String(name.to_string()));
    content.insert("payload".to_string(), payload);
    ToolResult::new(Value::Object(content))
}
