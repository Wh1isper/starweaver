//! Task result and text formatting helpers.

use std::{collections::BTreeSet, fmt::Write};

use serde_json::Value;
use starweaver_context::{Task, TaskStatus};
use starweaver_tools::ToolResult;

pub(super) fn task_result(name: &str, payload: Value, user_content: String) -> ToolResult {
    let mut content = serde_json::Map::new();
    content.insert("operation".to_string(), Value::String(name.to_string()));
    content.insert("payload".to_string(), payload);
    ToolResult::new(Value::Object(content)).with_user_content(Value::String(user_content))
}

pub(super) fn task_update_summary(task: &Task) -> String {
    let mut summary = String::new();
    let status = task.status.to_string();
    if !status.is_empty() {
        let _ = write!(summary, "status -> {status}");
    }
    if summary.is_empty() {
        summary.push_str(&task.subject);
    }
    summary
}

pub(super) fn task_list_text(tasks: &[Task]) -> String {
    if tasks.is_empty() {
        return "No tasks found.".to_string();
    }
    tasks
        .iter()
        .map(|task| task_summary_line(task, tasks))
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn task_detail_text(task: &Task) -> String {
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
    let status = if task.status == TaskStatus::InProgress {
        task.active_form.as_ref().map_or_else(
            || "in_progress".to_string(),
            |active| format!("in_progress: {active}"),
        )
    } else {
        task.status.to_string()
    };
    let mut line = format!("#{} [{}] {}", task.id, status, task.subject);
    let completed = all_tasks
        .iter()
        .filter(|candidate| candidate.status == TaskStatus::Completed)
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
