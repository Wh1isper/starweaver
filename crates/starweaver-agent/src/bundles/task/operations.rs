//! Task tool operations.

use std::collections::BTreeSet;

use serde_json::Value;
use starweaver_context::{AgentContextHandle, Task};
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
            let mut tasks = agent_context.tasks();
            let id = next_task_id(&tasks);
            let mut task = Task::new(id, subject, description);
            task.active_form = active_form;
            task.metadata = metadata;
            created = Some(task.clone());
            tasks.push(task);
            agent_context.set_tasks(tasks);
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

    if let Some(handle) = handle {
        handle.update(|agent_context| {
            let mut tasks = agent_context.tasks();
            if let Some(index) = tasks
                .iter()
                .position(|task| normalize_task_id(&task.id) == requested_id)
            {
                apply_task_update(&mut tasks, index, &arguments, &update_metadata);
                updated = tasks.get(index).cloned();
                agent_context.set_tasks(tasks);
            }
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

fn normalize_task_id(id: &str) -> String {
    id.trim().trim_start_matches('#').to_string()
}

fn next_task_id(tasks: &[Task]) -> String {
    let next = tasks
        .iter()
        .filter_map(|task| normalize_task_id(&task.id).parse::<u64>().ok())
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    next.to_string()
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

fn apply_task_update(
    tasks: &mut [Task],
    index: usize,
    arguments: &TaskUpdateArgs,
    metadata: &Metadata,
) {
    let task_id = normalize_task_id(&tasks[index].id);
    if let Some(status) = arguments
        .status
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        tasks[index].status = status.to_string();
    }
    if let Some(subject) = arguments
        .subject
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        tasks[index].subject = subject.to_string();
    }
    if let Some(description) = arguments
        .description
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        tasks[index].description = description.to_string();
    }
    if arguments.active_form.is_some() {
        tasks[index].active_form.clone_from(&arguments.active_form);
    }
    if arguments.owner.is_some() {
        tasks[index].owner.clone_from(&arguments.owner);
    }
    extend_unique(&mut tasks[index].blocks, arguments.add_blocks.as_deref());
    extend_unique(
        &mut tasks[index].blocked_by,
        arguments.add_blocked_by.as_deref(),
    );
    for (key, value) in metadata {
        tasks[index].metadata.insert(key.clone(), value.clone());
    }
    let blocks = arguments.add_blocks.clone().unwrap_or_default();
    let blocked_by = arguments.add_blocked_by.clone().unwrap_or_default();
    for blocked_task_id in blocks {
        let blocked_task_id = normalize_task_id(&blocked_task_id);
        if let Some(blocked_task) = tasks
            .iter_mut()
            .find(|task| normalize_task_id(&task.id) == blocked_task_id)
        {
            extend_unique(
                &mut blocked_task.blocked_by,
                Some(std::slice::from_ref(&task_id)),
            );
        }
    }
    for blocker_id in blocked_by {
        let blocker_id = normalize_task_id(&blocker_id);
        if let Some(blocker) = tasks
            .iter_mut()
            .find(|task| normalize_task_id(&task.id) == blocker_id)
        {
            extend_unique(&mut blocker.blocks, Some(std::slice::from_ref(&task_id)));
        }
    }
}

fn extend_unique(target: &mut Vec<String>, values: Option<&[String]>) {
    let Some(values) = values else {
        return;
    };
    let mut seen = target.iter().cloned().collect::<BTreeSet<_>>();
    for value in values {
        let normalized = normalize_task_id(value);
        if !normalized.is_empty() && seen.insert(normalized.clone()) {
            target.push(normalized);
        }
    }
}
