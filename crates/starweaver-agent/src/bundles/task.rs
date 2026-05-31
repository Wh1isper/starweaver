use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use starweaver_tools::{
    DynToolset, EmptyToolArgs, StaticToolset, ToolContext, ToolError, ToolInstruction, ToolResult,
};

use super::helpers::{static_tool_with_metadata, tool_metadata};

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
                static_tool_with_metadata(
                    "task_create",
                    "Create a new task. Task status defaults to pending.",
                    tool_metadata("task", true, false),
                    task_create,
                ),
                static_tool_with_metadata(
                    "task_get",
                    "Get task details by ID.",
                    tool_metadata("task", true, false),
                    task_get,
                ),
                static_tool_with_metadata(
                    "task_update",
                    "Update task status, content, or dependencies.",
                    tool_metadata("task", true, false),
                    task_update,
                ),
                static_tool_with_metadata(
                    "task_list",
                    "List all tasks and their status.",
                    tool_metadata("task", true, false),
                    task_list,
                ),
            ]),
    )
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct TaskCreateArgs {
    /// Task title in imperative form.
    subject: String,
    /// Detailed task description.
    description: String,
    /// Present progressive form shown during `in_progress`.
    #[serde(default)]
    active_form: Option<String>,
    /// Optional additional metadata.
    #[serde(default)]
    metadata: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct TaskIdArgs {
    /// The task ID to retrieve.
    task_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct TaskUpdateArgs {
    /// The task ID to update.
    task_id: String,
    /// New task status.
    #[serde(default)]
    status: Option<String>,
    /// New task title.
    #[serde(default)]
    subject: Option<String>,
    /// New task description.
    #[serde(default)]
    description: Option<String>,
    /// New present progressive form.
    #[serde(default)]
    active_form: Option<String>,
    /// Task owner or assignee.
    #[serde(default)]
    owner: Option<String>,
    /// Task IDs that this task blocks.
    #[serde(default)]
    add_blocks: Option<Vec<String>>,
    /// Task IDs that block this task.
    #[serde(default)]
    add_blocked_by: Option<Vec<String>>,
    /// Metadata to merge into task.
    #[serde(default)]
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
