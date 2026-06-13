use std::{collections::BTreeSet, fmt::Write, sync::Arc};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use starweaver_context::{AgentContextHandle, Task};
use starweaver_core::Metadata;
use starweaver_tools::{
    DynToolset, EmptyToolArgs, StaticToolset, ToolContext, ToolError, ToolInstruction, ToolResult,
};

use super::helpers::{static_tool_with_metadata, tool_metadata};

/// Create task operation tools.
#[must_use]
#[allow(clippy::needless_raw_string_hashes)]
pub fn task_tools() -> DynToolset {
    Arc::new(
        StaticToolset::new("task")
            .with_id("task")
            .with_instruction(ToolInstruction::new(
                "task-manager",
                r#"<task-manager-guidelines>

<overview>
Task management tools track multi-step work with dependencies. Use them for complex projects, breaking down work, and tracking progress.
</overview>

<tools>
- `task_create`: Create a new task (defaults to pending).
- `task_get`: Get task details by ID.
- `task_list`: List all tasks with status overview.
- `task_update`: Update task status, content, or dependencies.
</tools>

<workflow>
Status: pending -> in_progress -> completed.
- Set in_progress when starting work.
- Set completed immediately after finishing.
- Completed tasks automatically unblock dependents.
</workflow>

<dependencies>
- add_blocked_by: tasks that must complete before this one.
- add_blocks: tasks this one will block.
- Set up dependencies early when planning.
</dependencies>

<delegate-with-subagents>
Delegate calls are blocking -- the agent waits until the subagent finishes. Multiple delegate calls in the same response run concurrently.

1. Create tasks and identify which can run in parallel.
2. Call multiple delegates in a single response to run them concurrently.
3. When subagents return, update task status accordingly.

Sequential delegate calls across turns run serially.
</delegate-with-subagents>

</task-manager-guidelines>"#,
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

async fn task_get(context: ToolContext, arguments: TaskIdArgs) -> Result<ToolResult, ToolError> {
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

async fn task_update(
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

async fn task_list(
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

fn task_result(name: &str, payload: Value, user_content: String) -> ToolResult {
    let mut content = serde_json::Map::new();
    content.insert("operation".to_string(), Value::String(name.to_string()));
    content.insert("payload".to_string(), payload);
    ToolResult::new(Value::Object(content)).with_user_content(Value::String(user_content))
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

fn task_update_summary(task: &Task) -> String {
    let mut summary = String::new();
    if !task.status.is_empty() {
        let _ = write!(summary, "status -> {}", task.status);
    }
    if summary.is_empty() {
        summary.push_str(&task.subject);
    }
    summary
}

fn task_list_text(tasks: &[Task]) -> String {
    if tasks.is_empty() {
        return "No tasks found.".to_string();
    }
    tasks
        .iter()
        .map(|task| task_summary_line(task, tasks))
        .collect::<Vec<_>>()
        .join("\n")
}

fn task_detail_text(task: &Task) -> String {
    let mut lines = vec![task_summary_line(task, std::slice::from_ref(task))];
    if !task.description.trim().is_empty() {
        lines.push(format!("Description: {}", task.description));
    }
    if let Some(owner) = task
        .owner
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        lines.push(format!("Owner: {owner}"));
    }
    if !task.blocked_by.is_empty() {
        lines.push(format!("Blocked By: #{}", task.blocked_by.join(", #")));
    }
    if !task.blocks.is_empty() {
        lines.push(format!("Blocks: #{}", task.blocks.join(", #")));
    }
    lines.join("\n")
}

fn task_summary_line(task: &Task, all_tasks: &[Task]) -> String {
    let status = if task.status == "in_progress" {
        task.active_form.as_ref().map_or_else(
            || "in_progress".to_string(),
            |active| format!("in_progress: {active}"),
        )
    } else {
        task.status.clone()
    };
    let mut line = format!("#{} [{}] {}", task.id, status, task.subject);
    let completed = all_tasks
        .iter()
        .filter(|candidate| candidate.status == "completed")
        .map(|candidate| candidate.id.as_str())
        .collect::<BTreeSet<_>>();
    let active_blockers = task
        .blocked_by
        .iter()
        .filter(|blocker| !completed.contains(blocker.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    if !active_blockers.is_empty() {
        let _ = write!(line, " [blocked by #{}]", active_blockers.join(", #"));
    }
    line
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use serde_json::json;
    use starweaver_context::{AgentContext, AgentContextHandle, TASK_SNAPSHOT_EVENT_KIND};
    use starweaver_core::{AgentId, ConversationId, RunId};

    use super::task_tools;

    fn context_with_handle(handle: &AgentContextHandle) -> starweaver_tools::ToolContext {
        let mut dependencies = starweaver_context::DependencyStore::new();
        dependencies.insert(handle.clone());
        starweaver_tools::ToolContext::new(RunId::default(), ConversationId::default(), 0)
            .with_dependencies(dependencies)
    }

    #[tokio::test]
    async fn task_tools_mutate_context_and_emit_snapshots() {
        let handle = AgentContextHandle::new(AgentContext::new(AgentId::from_string("agent")));
        let toolset = task_tools();
        let create = toolset
            .get_tools()
            .into_iter()
            .find(|tool| tool.name() == "task_create")
            .unwrap();
        let update = toolset
            .get_tools()
            .into_iter()
            .find(|tool| tool.name() == "task_update")
            .unwrap();
        create
            .call(
                context_with_handle(&handle),
                json!({"subject": "ship", "description": "Ship release"}),
            )
            .await
            .unwrap();
        update
            .call(
                context_with_handle(&handle),
                json!({"task_id": "#1", "status": "in_progress", "active_form": "Shipping"}),
            )
            .await
            .unwrap();

        let context = handle.snapshot();
        let tasks = context.tasks();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].subject, "ship");
        assert_eq!(tasks[0].status, "in_progress");
        assert!(context
            .events
            .events()
            .iter()
            .any(|event| event.kind == TASK_SNAPSHOT_EVENT_KIND));
    }

    #[tokio::test]
    async fn failed_task_update_still_emits_current_snapshot() {
        let handle = AgentContextHandle::new(AgentContext::new(AgentId::from_string("agent")));
        let toolset = task_tools();
        let create = toolset
            .get_tools()
            .into_iter()
            .find(|tool| tool.name() == "task_create")
            .unwrap();
        let update = toolset
            .get_tools()
            .into_iter()
            .find(|tool| tool.name() == "task_update")
            .unwrap();
        create
            .call(
                context_with_handle(&handle),
                json!({"subject": "ship", "description": "Ship release"}),
            )
            .await
            .unwrap();
        let before_events = handle.snapshot().events.len();
        let result = update
            .call(
                context_with_handle(&handle),
                json!({"task_id": "#99", "status": "completed"}),
            )
            .await
            .unwrap();

        assert!(result
            .user_content
            .as_ref()
            .and_then(serde_json::Value::as_str)
            .unwrap()
            .contains("not found"));
        let context = handle.snapshot();
        assert_eq!(context.tasks().len(), 1);
        assert!(context.events.len() > before_events);
        assert_eq!(
            context.events.events().last().unwrap().kind,
            TASK_SNAPSHOT_EVENT_KIND
        );
        assert_eq!(
            context.events.events().last().unwrap().payload["tasks"][0]["id"],
            "1"
        );
    }
}
