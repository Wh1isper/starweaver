//! Model-facing runtime context rendering.

use chrono::Utc;
use starweaver_core::XmlWriter;

use crate::{AgentContext, TaskStatus};

/// Render runtime context instructions for model-facing requests.
#[must_use]
#[allow(clippy::unnecessary_wraps)]
pub fn render_runtime_context(context: &AgentContext, is_user_prompt: bool) -> Option<String> {
    let now = Utc::now();
    let elapsed_milliseconds = (now - context.started_at).num_milliseconds().max(0);
    let elapsed_tenths = (elapsed_milliseconds + 50) / 100;
    let elapsed = format!("{}.{:01}s", elapsed_tenths / 10, elapsed_tenths % 10);
    let mut xml = XmlWriter::new();
    xml.open("runtime-context")
        .text_element("agent-id", context.agent_id.as_str())
        .text_element("current-time", now.to_rfc3339())
        .text_element("elapsed-time", elapsed);

    if let Some(context_window) = context.model_config.context_window {
        xml.open("model-config")
            .text_element("context-window", context_window.to_string())
            .close("model-config");
    }

    let latest_total_tokens = context.latest_request_total_tokens();
    if let Some(total_tokens) = latest_total_tokens {
        xml.open("token-usage")
            .text_element("total-tokens", total_tokens.to_string())
            .close("token-usage");
    }

    append_known_subagents(&mut xml, context);
    append_active_tasks(&mut xml, context, is_user_prompt);

    if is_user_prompt && !context.notes.is_empty() {
        let entries = context.notes.entries();
        let count = entries.len().to_string();
        xml.open_attrs("notes", [("count", count.as_str())]);
        for (key, _value) in entries {
            xml.empty_element_attrs("note", [("key", key.as_str())]);
        }
        xml.close("notes");
    }

    xml.close("runtime-context");
    let mut output = xml.finish();
    if let Some(reminder) = context_pressure_reminder(context, latest_total_tokens) {
        let mut reminder_xml = XmlWriter::new();
        reminder_xml
            .open("system-reminder")
            .text_element("item", reminder)
            .close("system-reminder");
        output.push_str("\n\n");
        output.push_str(&reminder_xml.finish());
    }
    Some(output)
}

fn append_known_subagents(xml: &mut XmlWriter, context: &AgentContext) {
    let subagents = context
        .agent_registry
        .values()
        .filter(|agent| agent.agent_id != context.agent_id.as_str())
        .collect::<Vec<_>>();
    if subagents.is_empty() {
        return;
    }
    xml.open_attrs(
        "known-subagents",
        [("hint", "Use subagent_info tool for more details")],
    );
    for agent in subagents {
        let mut attrs = vec![
            ("id".to_string(), agent.agent_id.clone()),
            ("name".to_string(), agent.agent_name.clone()),
        ];
        if let Some(parent) = &agent.parent_agent_id {
            attrs.push(("parent-agent-id".to_string(), parent.clone()));
        }
        xml.empty_element_attrs(
            "agent",
            attrs
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str())),
        );
    }
    xml.close("known-subagents");
}

fn append_active_tasks(xml: &mut XmlWriter, context: &AgentContext, detailed: bool) {
    let tasks = context
        .tasks()
        .into_iter()
        .filter(|task| task.status != TaskStatus::Completed)
        .collect::<Vec<_>>();
    if tasks.is_empty() {
        return;
    }

    if detailed {
        xml.open_attrs(
            "active-tasks",
            [("hint", "Update status with task_update tool")],
        );
    } else {
        xml.open("active-tasks");
    }
    for task in &tasks {
        let active_blockers = task
            .blocked_by
            .iter()
            .filter(|blocked_by| {
                tasks.iter().any(|candidate| {
                    &candidate.id == *blocked_by && candidate.status != TaskStatus::Completed
                })
            })
            .map(String::as_str)
            .collect::<Vec<_>>();
        let mut attrs = vec![
            ("id".to_string(), task.id.clone()),
            ("status".to_string(), task.status.to_string()),
        ];
        if !active_blockers.is_empty() {
            attrs.push(("blocked-by".to_string(), active_blockers.join(",")));
        }

        if detailed {
            xml.open_attrs(
                "task",
                attrs
                    .iter()
                    .map(|(key, value)| (key.as_str(), value.as_str())),
            )
            .text_element("subject", &task.subject);
            if task.status == TaskStatus::InProgress {
                if let Some(active_form) = &task.active_form {
                    xml.text_element("active-form", active_form);
                }
            }
            xml.close("task");
        } else {
            xml.text_element_attrs(
                "task",
                attrs
                    .iter()
                    .map(|(key, value)| (key.as_str(), value.as_str())),
                &task.subject,
            );
        }
    }
    xml.close("active-tasks");
}

fn context_pressure_reminder(
    context: &AgentContext,
    latest_total_tokens: Option<u64>,
) -> Option<String> {
    let total_tokens = latest_total_tokens?;
    let context_window = context.model_config.context_window?;
    if context_window == 0 {
        return None;
    }
    let threshold = context
        .model_config
        .proactive_context_management_threshold?;
    if total_tokens.saturating_mul(1000)
        < context_window.saturating_mul(u64::from(threshold.per_thousand()))
    {
        return None;
    }
    let usage_pct = total_tokens.saturating_mul(100) / context_window;
    let compact_pct =
        u64::from(context.model_config.compact_threshold.per_thousand()).saturating_mul(100) / 1000;
    let mut reminder = format!(
        "Context usage is at {usage_pct}% ({} / {} tokens). Configured compact threshold is {compact_pct}%. Please summarize progress and continue with a smaller context when appropriate.",
        format_u64_with_commas(total_tokens),
        format_u64_with_commas(context_window),
    );
    if !context.context_manage_tool_names.is_empty() {
        reminder.push_str(" Available context management tools: ");
        reminder.push_str(&context.context_manage_tool_names.join(", "));
        reminder.push('.');
    }
    if !context.notes.is_empty() {
        reminder.push_str(
            " Review note keys, read needed values, and delete stale or oversized notes before summarizing.",
        );
    }
    Some(reminder)
}

fn format_u64_with_commas(value: u64) -> String {
    let digits = value.to_string();
    let mut output = String::with_capacity(digits.len() + digits.len() / 3);
    let first_group_len = digits.len() % 3;
    let mut index = 0usize;
    if first_group_len > 0 {
        output.push_str(&digits[..first_group_len]);
        index = first_group_len;
        if index < digits.len() {
            output.push(',');
        }
    }
    while index < digits.len() {
        output.push_str(&digits[index..index + 3]);
        index += 3;
        if index < digits.len() {
            output.push(',');
        }
    }
    output
}
