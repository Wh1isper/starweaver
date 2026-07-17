use std::{fmt::Write as _, path::Path};

use serde_json::Value;
use starweaver_context::TASK_SNAPSHOT_EVENT_KIND;
use starweaver_model::{PartDelta, StreamDelta};
use starweaver_runtime::{AgentStreamEvent, AgentStreamRecord, ModelResponseStreamEvent};
use starweaver_usage::Usage;

use super::{
    ModelChoice, SHELL_OUTPUT_MAX_LINES, SHELL_STREAM_PREVIEW_MAX_LINES, StreamingPartKind,
    StreamingToolCallState, TOOL_PREVIEW_MAX_CHARS, TaskPanelItem, ToolConciseSummary,
    ToolSummaryCategory, ToolSummaryImportance, ToolVisibility,
};
use crate::tui::{
    markdown::ASSISTANT_CONTENT_PREFIX,
    render::{truncate_line_center, value_preview},
};

mod context_events;
mod tasks;
mod tool_returns;
mod tool_summaries;

pub(super) use context_events::{
    format_custom_context_event_lines, format_subagent_finished_line, format_subagent_running_line,
    is_subagent_lifecycle_event_kind, is_subagent_start_event_kind, normalized_event_kind,
    subagent_display_id,
};
use context_events::{payload_string, payload_string_array, push_indented_preview};
use tasks::format_task_tool_lines;
pub(super) use tasks::{is_task_snapshot_event, is_task_tool_name, task_panel_items_from_value};
pub(super) use tool_summaries::{
    format_streaming_tool_summary, format_tool_call_summary, format_tool_call_summary_from_parts,
    format_tool_return_summary,
};

pub(super) fn push_usage_entry_lines(lines: &mut Vec<String>, name: &str, usage: &Usage) {
    lines.push(format!("[SYS]   {name}:"));
    lines.push(format!(
        "[SYS]     Input:  {} tokens",
        format_u64_with_commas(usage.input_tokens)
    ));
    lines.push(format!(
        "[SYS]     Output: {} tokens",
        format_u64_with_commas(usage.output_tokens)
    ));
    if usage.cache_write_tokens > 0 {
        lines.push(format!(
            "[SYS]     Cache Write: {} tokens",
            format_u64_with_commas(usage.cache_write_tokens)
        ));
    }
    if usage.cache_read_tokens > 0 {
        lines.push(format!(
            "[SYS]     Cache Read:  {} tokens",
            format_u64_with_commas(usage.cache_read_tokens)
        ));
    }
    if let Some(cache_hit_rate) = cache_hit_rate_label(usage) {
        lines.push(format!("[SYS]     Cache Hit Rate: {cache_hit_rate}"));
    }
    lines.push(format!("[SYS]     Requests: {}", usage.requests));
    if usage.tool_calls > 0 {
        lines.push(format!("[SYS]     Tool calls: {}", usage.tool_calls));
    }
}

pub(super) fn cache_hit_rate_label(usage: &Usage) -> Option<String> {
    if usage.input_tokens == 0 || usage.cache_read_tokens == 0 {
        return None;
    }
    let basis_points = usage
        .cache_read_tokens
        .saturating_mul(10_000)
        .saturating_add(usage.input_tokens / 2)
        / usage.input_tokens;
    Some(format!("{}.{:02}%", basis_points / 100, basis_points % 100))
}

pub(super) fn format_u64_with_commas(value: u64) -> String {
    let text = value.to_string();
    let mut output = String::with_capacity(text.len() + text.len() / 3);
    for (index, ch) in text.chars().rev().enumerate() {
        if index > 0 && index % 3 == 0 {
            output.push(',');
        }
        output.push(ch);
    }
    output.chars().rev().collect()
}

pub(super) fn assistant_content_line(line: impl AsRef<str>) -> String {
    format!("{ASSISTANT_CONTENT_PREFIX}{}", line.as_ref())
}

pub(super) fn streaming_part_kind(part_kind: &str) -> StreamingPartKind {
    let normalized = part_kind.to_ascii_lowercase();
    if normalized.contains("thinking") || normalized.contains("reasoning") {
        StreamingPartKind::Thinking
    } else if normalized.contains("tool") || normalized.contains("function_call") {
        StreamingPartKind::ToolCall
    } else if normalized.contains("text") || normalized.contains("message") {
        StreamingPartKind::Text
    } else {
        StreamingPartKind::Other
    }
}

pub(super) fn merge_stream_fragment(current: Option<&str>, fragment: &str) -> String {
    match current {
        Some(current) if !current.is_empty() && fragment.starts_with(current) => {
            fragment.to_string()
        }
        Some(current) => format!("{current}{fragment}"),
        None => fragment.to_string(),
    }
}

pub(super) fn format_streaming_tool_call_line(state: Option<&StreamingToolCallState>) -> String {
    let name = state
        .and_then(|state| state.name.as_deref())
        .filter(|name| !name.is_empty())
        .unwrap_or("tool");
    let arguments = state.map_or("", |state| state.arguments.trim());
    if arguments.is_empty() || arguments == "{}" || arguments == "null" {
        format!("Tool call: {name}")
    } else {
        format!("Tool call: {name} {}", truncate_line_center(arguments, 80))
    }
}

pub(super) fn tool_call_visibility_key(call: &starweaver_model::ToolCallPart) -> String {
    if call.id.is_empty() {
        format!(
            "{}:{}",
            call.name,
            value_preview(&call.arguments.replay_value())
        )
    } else {
        call.id.clone()
    }
}

pub(super) fn model_choice_label(choice: &ModelChoice) -> String {
    if choice.display_name() == choice.model_id {
        choice.model_id.clone()
    } else {
        format!("{} ({})", choice.display_name(), choice.model_id)
    }
}

pub(super) fn model_choice_config_suffix(choice: &ModelChoice) -> String {
    let mut parts = Vec::new();
    if let Some(settings) = choice.model_settings.as_deref() {
        parts.push(format!("settings={settings}"));
    }
    if let Some(config) = choice.model_cfg.as_deref() {
        parts.push(format!("cfg={config}"));
    }
    if let Some(window) = choice.context_window {
        parts.push(format!("context={window}"));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" [{}]", parts.join(" "))
    }
}

pub(in crate::tui) fn display_lines_for_stream_record(record: &AgentStreamRecord) -> Vec<String> {
    match &record.event {
        AgentStreamEvent::ModelStream {
            event: ModelResponseStreamEvent::PartDelta(PartDelta { delta, .. }),
            ..
        } => match delta {
            StreamDelta::Thinking { text } => text
                .lines()
                .map(|line| assistant_content_line(format!("> {line}")))
                .collect(),
            StreamDelta::Text { text } => text.lines().map(assistant_content_line).collect(),
            _ => Vec::new(),
        },
        AgentStreamEvent::ToolCall { call, .. } => vec![format_tool_call_line(call)],
        AgentStreamEvent::ToolReturn { tool_return, .. } => {
            format_tool_return_lines(tool_return, None)
        }
        AgentStreamEvent::Custom { event } if event.kind == "steering_submitted" => event
            .payload
            .get("text")
            .and_then(Value::as_str)
            .map_or_else(Vec::new, |text| vec![format!("Steering: {text}")]),
        AgentStreamEvent::Custom { event } if event.kind == "steering_received" => event
            .payload
            .get("text")
            .and_then(Value::as_str)
            .map_or_else(
                || vec!["Steering received".to_string()],
                |text| vec![format!("Steering received: {text}")],
            ),
        AgentStreamEvent::Custom { event } if is_subagent_lifecycle_event_kind(&event.kind) => {
            let normalized = normalized_event_kind(&event.kind);
            if context_events::is_subagent_start_event_kind(&normalized) {
                vec![format_subagent_running_line(&event.payload)]
            } else {
                vec![format_subagent_finished_line(&event.kind, &event.payload)]
            }
        }
        AgentStreamEvent::Custom { event } => {
            format_custom_context_event_lines(&event.kind, &event.payload).unwrap_or_default()
        }
        AgentStreamEvent::RunFailed { message, .. } => vec![format!("Run failed: {message}")],
        _ => Vec::new(),
    }
}

pub(super) fn format_tool_call_line(call: &starweaver_model::ToolCallPart) -> String {
    if is_task_tool_name(&call.name) {
        return format!("Task request: {}", call.name);
    }
    let arguments = tool_call_arguments_text(call);
    if arguments == "{}" || arguments == "null" || arguments.is_empty() {
        format!("Tool call: {}", call.name)
    } else {
        format!("Tool call: {} {arguments}", call.name)
    }
}

fn tool_call_arguments_text(call: &starweaver_model::ToolCallPart) -> String {
    let value = call.arguments.replay_value();
    if call.name == "shell_exec" {
        full_value_text(&value)
    } else {
        value_preview(&value)
    }
}

pub(super) fn format_tool_return_lines(
    tool_return: &starweaver_model::ToolReturnPart,
    arguments: Option<&Value>,
) -> Vec<String> {
    tool_returns::format_tool_return_display_lines(tool_return, arguments)
}

pub(super) fn string_field<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or_default()
}

fn preview_lines(content: &str, max_lines: usize) -> Vec<String> {
    let lines = content.lines().collect::<Vec<_>>();
    let mut preview = lines
        .iter()
        .take(max_lines)
        .map(|line| preview_line(line))
        .collect::<Vec<_>>();
    if lines.len() > max_lines {
        preview.push(format!("... ({} more lines)", lines.len() - max_lines));
    }
    preview
}

fn full_content_lines(content: &str) -> Vec<String> {
    let mut lines = content
        .lines()
        .map(sanitize_control_chars)
        .collect::<Vec<_>>();
    if content.ends_with('\n') {
        lines.push(String::new());
    }
    lines
}

fn preview_line(line: &str) -> String {
    truncate_line_center(&sanitize_control_chars(line), TOOL_PREVIEW_MAX_CHARS)
}

fn sanitize_control_chars(text: &str) -> String {
    let mut sanitized = String::new();
    for ch in text.chars() {
        match ch {
            '\t' => sanitized.push(ch),
            '\r' => sanitized.push_str("\\r"),
            '\x1b' => sanitized.push_str("\\x1b"),
            ch if ch.is_control() => {
                let _ = write!(&mut sanitized, "\\x{:02x}", u32::from(ch));
            }
            ch => sanitized.push(ch),
        }
    }
    sanitized
}

const fn plural_suffix(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

fn is_empty_result(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(value) => value.trim().is_empty(),
        other => other == &Value::Bool(true) || other == &Value::Object(serde_json::Map::new()),
    }
}

fn value_text(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        other => other.to_string(),
    }
}

fn full_value_text(value: &Value) -> String {
    sanitize_control_chars(&value_text(value).replace('\n', " "))
}

pub(super) fn streaming_tool_state_is_available(state: &StreamingToolCallState, key: &str) -> bool {
    state.item_id.is_some()
        && state
            .linked_call_key
            .as_deref()
            .is_none_or(|linked| linked == key)
}

pub(super) fn streaming_tool_arguments_match(
    streamed_arguments: &str,
    call: &starweaver_model::ToolCallPart,
) -> bool {
    let streamed_arguments = streamed_arguments.trim();
    let final_wire = call.arguments.wire_json_string();
    if streamed_arguments == final_wire
        || streamed_arguments == value_preview(&call.arguments.replay_value())
    {
        return true;
    }
    if streamed_arguments.is_empty() {
        return matches!(final_wire.trim(), "" | "{}" | "null");
    }
    serde_json::from_str::<serde_json::Value>(streamed_arguments).is_ok_and(|streamed| {
        streamed == call.arguments.execution_value() || streamed == call.arguments.replay_value()
    })
}

pub(super) fn body_line_display_text(line: &str) -> &str {
    line.strip_prefix(ASSISTANT_CONTENT_PREFIX)
        .unwrap_or(line)
        .trim()
}

pub(super) fn compact_status_text(text: &str, max_chars: usize) -> String {
    let compact = text.replace('\n', " ");
    let char_count = compact.chars().count();
    if char_count <= max_chars {
        return compact;
    }
    let keep = max_chars.saturating_sub(1);
    let suffix = compact
        .chars()
        .take(keep)
        .collect::<String>()
        .trim_end()
        .to_string();
    format!("{suffix}…")
}

pub(super) fn pasted_image_paths(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|part| part.trim_matches(['\'', '"']))
        .filter(|part| {
            Path::new(part).extension().is_some_and(|extension| {
                ["png", "jpg", "jpeg", "webp", "gif"]
                    .iter()
                    .any(|image_extension| extension.eq_ignore_ascii_case(image_extension))
            })
        })
        .map(str::to_string)
        .collect()
}

pub(super) fn push_shell_output_lines(body: &mut Vec<String>, label: &str, output: &str) {
    if output.trim().is_empty() {
        return;
    }
    body.push(format!("Shell {label}:"));
    for line in output.lines().take(SHELL_OUTPUT_MAX_LINES) {
        body.push(format!("  {line}"));
    }
    if output.lines().count() > SHELL_OUTPUT_MAX_LINES {
        body.push(format!(
            "[SYS] {label} truncated to {SHELL_OUTPUT_MAX_LINES} lines"
        ));
    }
}
