//! Task tool operations.

use serde_json::Value;
use starweaver_context::{AgentContextHandle, Task, TaskStatus};
use starweaver_core::Metadata;
use starweaver_tools::{EmptyToolArgs, ToolContext, ToolError, ToolResult};

use super::args::{TaskCreateArgs, TaskIdArgs, TaskUpdateArgs};
use super::formatting::{task_detail_text, task_list_text, task_result, task_update_summary};

pub(super) async fn task_create(
    context: ToolContext,
    arguments: TaskCreateArgs,
) -> Result<ToolResult, ToolError> {
    let subject = arguments.subject;
    let description = arguments.description;
    let active_form = arguments.active_form;
    let metadata = value_to_metadata(arguments.metadata);
    let handle = context.dependency::<AgentContextHandle>();
    let mut created = None;
    let mut snapshot = Vec::new();

    if let Some(handle) = handle {
        handle.update(|agent_context| {
            let task =
                agent_context
                    .task_manager
                    .create(subject, description, active_form, metadata);
            created = Some(task);
            sync_task_snapshot(agent_context);
            snapshot = agent_context.tasks();
            agent_context.publish_task_snapshot_event();
        });
    } else {
        let mut task = Task::new("1", subject, description);
        task.active_form = active_form;
        task.metadata = metadata;
        created = Some(task.clone());
        snapshot = vec![task];
    }

    let task = created.ok_or_else(|| ToolError::Execution {
        tool: "task_create".to_string(),
        message: "task was not created".to_string(),
    })?;
    Ok(task_result(
        "task_create",
        serde_json::json!({"task": task, "tasks": snapshot}),
        format!("Task #{} created successfully: {}", task.id, task.subject),
    ))
}

pub(super) async fn task_get(
    context: ToolContext,
    arguments: TaskIdArgs,
) -> Result<ToolResult, ToolError> {
    let requested_id = normalize_task_id(&arguments.task_id);
    let tasks = with_task_snapshot_event(&context);
    let task = tasks
        .iter()
        .find(|task| normalize_task_id(&task.id) == requested_id)
        .cloned();
    let payload = serde_json::json!({
        "task_id": requested_id,
        "task": task,
        "tasks": tasks,
    });
    let user_content = task.as_ref().map_or_else(
        || format!("Task #{requested_id} not found"),
        task_detail_text,
    );
    Ok(task_result("task_get", payload, user_content))
}

pub(super) async fn task_update(
    context: ToolContext,
    arguments: TaskUpdateArgs,
) -> Result<ToolResult, ToolError> {
    let requested_id = normalize_task_id(&arguments.task_id);
    let handle = context.dependency::<AgentContextHandle>();
    let mut updated = None;
    let mut snapshot = Vec::new();
    let update_metadata = value_to_metadata(arguments.metadata.clone());
    let parsed_status = match arguments
        .status
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        Some(status) => {
            Some(
                TaskStatus::parse(status).ok_or_else(|| ToolError::InvalidArguments {
                    tool: "task_update".to_string(),
                    message: format!(
                "invalid task status '{status}'; expected one of: pending, in_progress, completed"
            ),
                })?,
            )
        }
        None => None,
    };

    if let Some(handle) = handle {
        handle.update(|agent_context| {
            updated = agent_context.task_manager.update(
                &requested_id,
                parsed_status.clone(),
                arguments.subject.clone(),
                arguments.description.clone(),
                arguments.active_form.clone().map(Some),
                arguments.owner.clone().map(Some),
                arguments.add_blocks.as_deref(),
                arguments.add_blocked_by.as_deref(),
                Some(&update_metadata),
            );
            sync_task_snapshot(agent_context);
            snapshot = agent_context.tasks();
            agent_context.publish_task_snapshot_event();
        });
    }

    let payload = serde_json::json!({
        "task_id": requested_id,
        "task": updated,
        "tasks": snapshot,
    });
    let user_content = updated.as_ref().map_or_else(
        || format!("Task #{requested_id} not found"),
        |task| format!("Updated task #{}: {}", task.id, task_update_summary(task)),
    );
    Ok(task_result("task_update", payload, user_content))
}

pub(super) async fn task_list(
    context: ToolContext,
    _arguments: EmptyToolArgs,
) -> Result<ToolResult, ToolError> {
    let tasks = with_task_snapshot_event(&context);
    let user_content = task_list_text(&tasks);
    Ok(task_result(
        "task_list",
        serde_json::json!({"tasks": tasks}),
        user_content,
    ))
}

fn with_task_snapshot_event(context: &ToolContext) -> Vec<Task> {
    let Some(handle) = context.dependency::<AgentContextHandle>() else {
        return Vec::new();
    };
    let mut tasks = Vec::new();
    handle.update(|agent_context| {
        tasks = agent_context.tasks();
        agent_context.publish_task_snapshot_event();
    });
    tasks
}

fn sync_task_snapshot(agent_context: &mut starweaver_context::AgentContext) {
    let tasks = agent_context.task_manager.list_all();
    agent_context.set_tasks(tasks);
}

fn normalize_task_id(id: &str) -> String {
    id.trim().trim_start_matches('#').to_string()
}

fn value_to_metadata(value: Option<Value>) -> Metadata {
    match value {
        Some(Value::Object(map)) => map,
        Some(value) if !value.is_null() => {
            let mut metadata = Metadata::default();
            metadata.insert("value".to_string(), value);
            metadata
        }
        _ => Metadata::default(),
    }
}
