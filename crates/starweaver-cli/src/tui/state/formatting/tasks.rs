use super::{sanitize_control_chars, value_text, TaskPanelItem, Value, TASK_SNAPSHOT_EVENT_KIND};

pub(in crate::tui::state) fn is_task_tool_name(name: &str) -> bool {
    matches!(
        name,
        "task_create" | "task_get" | "task_update" | "task_list"
    )
}

pub(super) fn format_task_tool_lines(
    name: &str,
    structured: &Value,
    display_value: &Value,
) -> Vec<String> {
    let payload = structured.get("payload").unwrap_or(structured);
    let task_payload = payload
        .get("task")
        .filter(|value| value.is_object())
        .unwrap_or(payload);
    let content = value_text(display_value);
    let mut lines = vec![format!("Task result: {name}")];
    match name {
        "task_create" => {
            lines.push("  Summary: Task created".to_string());
            let pushed = push_task_payload_fields(
                &mut lines,
                task_payload,
                &[
                    ("Task ID", &["id", "task_id"]),
                    ("Subject", &["subject"]),
                    ("Description", &["description"]),
                    ("Active form", &["active_form"]),
                    ("Owner", &["owner"]),
                    ("Metadata", &["metadata"]),
                ],
            );
            if !pushed {
                push_task_display_output(&mut lines, &content);
            }
        }
        "task_update" => {
            lines.push("  Summary: Task updated".to_string());
            let pushed = push_task_payload_fields(
                &mut lines,
                task_payload,
                &[
                    ("Task ID", &["task_id", "id"]),
                    ("Status", &["status"]),
                    ("Subject", &["subject"]),
                    ("Description", &["description"]),
                    ("Active form", &["active_form"]),
                    ("Owner", &["owner"]),
                    ("Blocks", &["add_blocks", "blocks"]),
                    ("Blocked by", &["add_blocked_by", "blocked_by"]),
                    ("Metadata", &["metadata"]),
                ],
            );
            if !pushed {
                push_task_display_output(&mut lines, &content);
            }
        }
        "task_get" => {
            lines.push("  Summary: Task details requested".to_string());
            let pushed = push_task_payload_fields(
                &mut lines,
                task_payload,
                &[("Task ID", &["task_id", "id"]), ("Metadata", &["metadata"])],
            );
            if !pushed || !content.trim_start().starts_with("Task #") {
                push_task_display_output(&mut lines, &content);
            }
        }
        "task_list" => format_task_list_lines(&mut lines, &content),
        _ => push_task_display_output(&mut lines, &content),
    }
    lines
}

fn format_task_list_lines(lines: &mut Vec<String>, content: &str) {
    let task_lines = content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();
    if task_lines.is_empty() {
        lines.push("  Summary: No tasks found".to_string());
        return;
    }
    let task_entries = task_lines
        .iter()
        .filter(|line| is_task_status_line(line))
        .collect::<Vec<_>>();
    if task_entries.is_empty() && task_lines.len() == 1 && task_lines[0] == "Task list requested" {
        lines.push("  Summary: Task list requested".to_string());
        return;
    }
    lines.push("  Output:".to_string());
    for line in &task_lines {
        lines.push(format!("    │ {}", sanitize_control_chars(line)));
    }
    if !task_entries.is_empty() {
        let completed = task_entries
            .iter()
            .filter(|line| line.contains("[completed]"))
            .count();
        let in_progress = task_entries
            .iter()
            .filter(|line| line.contains("[in_progress"))
            .count();
        lines.push(format!(
            "  Progress: {}/{}{}",
            completed,
            task_entries.len(),
            if in_progress > 0 {
                format!(" ({in_progress} in progress)")
            } else {
                String::new()
            }
        ));
    }
}

fn push_task_payload_fields(
    lines: &mut Vec<String>,
    payload: &Value,
    fields: &[(&str, &[&str])],
) -> bool {
    let mut pushed = false;
    for (label, keys) in fields {
        if let Some(value) = keys.iter().find_map(|key| payload.get(*key)) {
            pushed |= push_task_field(lines, label, value);
        }
    }
    pushed
}

fn push_task_field(lines: &mut Vec<String>, label: &str, value: &Value) -> bool {
    if task_field_is_empty(value) {
        return false;
    }
    match value {
        Value::String(text) if text.contains('\n') => {
            lines.push(format!("  {label}:"));
            for line in text.lines() {
                lines.push(format!("    │ {}", sanitize_control_chars(line)));
            }
        }
        Value::String(text) => {
            lines.push(format!("  {label}: {}", sanitize_control_chars(text)));
        }
        Value::Array(items) if items.iter().all(Value::is_string) => {
            let values = items
                .iter()
                .filter_map(Value::as_str)
                .map(sanitize_control_chars)
                .collect::<Vec<_>>();
            lines.push(format!("  {label}: {}", values.join(", ")));
        }
        other => {
            lines.push(format!("  {label}:"));
            let text = serde_json::to_string_pretty(other).unwrap_or_else(|_| other.to_string());
            for line in text.lines() {
                lines.push(format!("    │ {}", sanitize_control_chars(line)));
            }
        }
    }
    true
}

fn task_field_is_empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(text) => text.trim().is_empty(),
        Value::Array(items) => items.is_empty(),
        Value::Object(items) => items.is_empty(),
        _ => false,
    }
}

fn push_task_display_output(lines: &mut Vec<String>, content: &str) {
    if content.trim().is_empty() {
        return;
    }
    lines.push("  Output:".to_string());
    for line in content.lines() {
        lines.push(format!("    │ {}", sanitize_control_chars(line)));
    }
}

fn is_task_status_line(line: &str) -> bool {
    ["[pending]", "[in_progress", "[completed]"]
        .iter()
        .any(|status| line.contains(status))
}

pub(in crate::tui::state) fn is_task_snapshot_event(kind: &str) -> bool {
    let normalized = kind.to_ascii_lowercase().replace(['.', '-'], "_");
    normalized == TASK_SNAPSHOT_EVENT_KIND
        || normalized == "task_panel"
        || normalized.ends_with("_task_snapshot")
        || normalized.ends_with("_task_panel")
}

pub(in crate::tui::state) fn task_panel_items_from_value(
    value: &Value,
) -> Option<Vec<TaskPanelItem>> {
    let payload = value.get("payload").unwrap_or(value);
    for candidate in [payload, value] {
        if let Some(items) = candidate
            .get("tasks")
            .or_else(|| candidate.get("items"))
            .and_then(Value::as_array)
            .or_else(|| candidate.as_array())
        {
            return Some(
                items
                    .iter()
                    .filter_map(task_panel_item_from_value)
                    .collect(),
            );
        }
    }
    for candidate in [payload, value] {
        if let Some(task) = candidate.get("task").filter(|task| task.is_object()) {
            if let Some(item) = task_panel_item_from_value(task) {
                return Some(vec![item]);
            }
        }
        if candidate.is_object() {
            if let Some(item) = task_panel_item_from_value(candidate) {
                return Some(vec![item]);
            }
        }
    }
    None
}

fn task_panel_item_from_value(value: &Value) -> Option<TaskPanelItem> {
    let task = value
        .get("task")
        .filter(|task| task.is_object())
        .unwrap_or(value);
    Some(TaskPanelItem {
        id: task
            .get("id")
            .or_else(|| task.get("task_id"))
            .and_then(Value::as_str)?
            .trim_start_matches('#')
            .to_string(),
        subject: task
            .get("subject")
            .and_then(Value::as_str)
            .unwrap_or("untitled")
            .to_string(),
        description: task
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        status: task
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("pending")
            .to_string(),
        active_form: task
            .get("active_form")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string),
        owner: task
            .get("owner")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string),
        blocked_by: value_string_vec(task.get("blocked_by")),
        blocks: value_string_vec(task.get("blocks")),
    })
}

fn value_string_vec(value: Option<&Value>) -> Vec<String> {
    let Some(value) = value else {
        return Vec::new();
    };
    match value {
        Value::Array(items) => items
            .iter()
            .filter_map(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(|value| value.trim_start_matches('#').to_string())
            .collect(),
        Value::String(text) if !text.trim().is_empty() => {
            vec![text.trim_start_matches('#').to_string()]
        }
        _ => Vec::new(),
    }
}
