use std::{fmt::Write, sync::Arc};

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
    let user_content = task_user_content(name, &payload);
    let mut content = serde_json::Map::new();
    content.insert("operation".to_string(), Value::String(name.to_string()));
    content.insert("payload".to_string(), payload);
    ToolResult::new(Value::Object(content)).with_user_content(Value::String(user_content))
}

fn task_user_content(name: &str, payload: &Value) -> String {
    match name {
        "task_create" => payload
            .get("subject")
            .and_then(Value::as_str)
            .filter(|subject| !subject.trim().is_empty())
            .map_or_else(
                || "Task created".to_string(),
                |subject| format!("Task created: {subject}"),
            ),
        "task_get" => payload
            .get("task_id")
            .and_then(Value::as_str)
            .filter(|task_id| !task_id.trim().is_empty())
            .map_or_else(
                || "Task not found".to_string(),
                |task_id| format!("Task #{task_id}"),
            ),
        "task_update" => {
            let task_id = payload
                .get("task_id")
                .and_then(Value::as_str)
                .filter(|task_id| !task_id.trim().is_empty());
            let mut text = task_id.map_or_else(
                || "Task updated".to_string(),
                |task_id| format!("Task updated: #{task_id}"),
            );
            if let Some(status) = payload
                .get("status")
                .and_then(Value::as_str)
                .filter(|status| !status.trim().is_empty())
            {
                let _ = write!(text, " [{status}]");
            }
            if let Some(subject) = payload
                .get("subject")
                .and_then(Value::as_str)
                .filter(|subject| !subject.trim().is_empty())
            {
                text.push(' ');
                text.push_str(subject);
            }
            text
        }
        "task_list" => "Task list requested".to_string(),
        _ => "No task data".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{operation, task_user_content};

    #[test]
    fn task_operation_preserves_machine_content_and_adds_user_content() {
        let result = operation(
            "task_update",
            json!({
                "task_id": "6",
                "status": "completed",
                "subject": "Align task display"
            }),
        );

        assert_eq!(result.content["operation"], "task_update");
        assert_eq!(result.content["payload"]["task_id"], "6");
        assert_eq!(
            result.user_content,
            Some(json!("Task updated: #6 [completed] Align task display"))
        );
    }

    #[test]
    fn task_user_content_matches_concise_cli_display() {
        assert_eq!(
            task_user_content("task_create", &json!({"subject": "Run tests"})),
            "Task created: Run tests"
        );
        assert_eq!(
            task_user_content("task_get", &json!({"task_id": "8"})),
            "Task #8"
        );
        assert_eq!(
            task_user_content("task_list", &json!({})),
            "Task list requested"
        );
    }
}
