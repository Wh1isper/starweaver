//! Terminal UI rendering built from display messages.

use std::fmt::Write as _;

use serde::Serialize;
use serde_json::Value;
use starweaver_context::TASK_SNAPSHOT_EVENT_KIND;
use starweaver_model::{ToolArguments, ToolCallPart, ToolReturnPart};
use starweaver_runtime::{AgentStreamEvent, AgentStreamRecord};
use starweaver_session::{ApprovalRecord, DeferredToolRecord};
use starweaver_stream::{DisplayMessage, DisplayMessageKind};

use super::state::{display_lines_for_stream_record, TaskPanelItem};

/// Non-interactive TUI snapshot used by the renderer and tests.
#[derive(Clone, Debug, Default, Serialize)]
pub struct TuiSnapshot {
    /// Session id rendered by the snapshot.
    pub session_id: String,
    /// Display message count.
    pub messages: usize,
    /// Rendered assistant text preview.
    pub assistant_text: String,
    /// Tool call previews.
    pub tool_calls: Vec<String>,
    /// Steering message previews.
    pub steering: Vec<String>,
    /// Pending approval count.
    pub pending_approvals: usize,
    /// Pending deferred tool count.
    pub pending_deferred: usize,
    /// Terminal status if seen.
    pub terminal_status: Option<String>,
    /// Latest task board snapshot.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tasks: Vec<TaskPanelItem>,
    /// Transcript lines reconstructed in durable display-message order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transcript_lines: Vec<String>,
}

impl TuiSnapshot {
    /// Build a snapshot from persisted display messages and control-flow records.
    #[must_use]
    pub fn from_parts(
        session_id: String,
        messages: Vec<DisplayMessage>,
        approvals: &[ApprovalRecord],
        deferred: &[DeferredToolRecord],
    ) -> Self {
        let mut snapshot = Self {
            session_id,
            messages: messages.len(),
            pending_approvals: approvals
                .iter()
                .filter(|approval| approval.status == starweaver_session::ApprovalStatus::Pending)
                .count(),
            pending_deferred: deferred
                .iter()
                .filter(|record| {
                    matches!(
                        record.status,
                        starweaver_session::ExecutionStatus::Pending
                            | starweaver_session::ExecutionStatus::Waiting
                    )
                })
                .count(),
            ..Self::default()
        };
        let mut ordered_messages = messages;
        ordered_messages.sort_by_key(|message| message.sequence);
        let mut next_sequence = 0;
        for message in ordered_messages {
            if let Some(record) = display_message_to_stream_record(&message, next_sequence) {
                snapshot
                    .transcript_lines
                    .extend(display_lines_for_stream_record(&record));
                next_sequence = next_sequence.saturating_add(1);
            }
            snapshot.apply_message(&message);
        }
        snapshot
    }

    /// Apply one display message to the retained view snapshot.
    pub fn apply_message(&mut self, message: &DisplayMessage) {
        match message.kind {
            DisplayMessageKind::AssistantTextDelta => self.apply_assistant_delta(message),
            DisplayMessageKind::ToolCallStart | DisplayMessageKind::ToolCallDelta => {
                self.apply_tool_call(message);
            }
            DisplayMessageKind::ToolResult => self.apply_tool_result(message),
            DisplayMessageKind::SteeringSubmitted | DisplayMessageKind::SteeringReceived => {
                self.apply_steering(message);
            }
            DisplayMessageKind::TaskSnapshot => self.apply_task_snapshot(message),
            DisplayMessageKind::RunCompleted => {
                self.terminal_status = Some("completed".to_string());
            }
            DisplayMessageKind::RunFailed => {
                self.terminal_status = Some("failed".to_string());
            }
            DisplayMessageKind::RunCancelled => {
                self.terminal_status = Some("cancelled".to_string());
            }
            _ => {}
        }
    }

    fn apply_assistant_delta(&mut self, message: &DisplayMessage) {
        let content = message
            .payload
            .get("delta")
            .or_else(|| message.payload.get("text"))
            .and_then(Value::as_str)
            .or(message.preview.as_deref());
        if message.payload.get("part_kind").and_then(Value::as_str) == Some("thinking")
            || message
                .metadata
                .get("reasoning")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        {
            if let Some(content) = content {
                append_blockquote_text(&mut self.assistant_text, content);
            }
            return;
        }
        if let Some(content) = content {
            self.assistant_text.push_str(content);
        }
    }

    fn apply_tool_call(&mut self, message: &DisplayMessage) {
        let name = message
            .payload
            .get("tool_name")
            .or_else(|| message.payload.get("name"))
            .and_then(Value::as_str)
            .or(message.preview.as_deref())
            .unwrap_or("tool");
        let arguments = message
            .payload
            .get("arguments")
            .map(value_preview)
            .or_else(|| string_payload(message, "delta"))
            .or_else(|| string_payload(message, "arguments_delta"));
        if let Some(arguments) =
            arguments.filter(|value| !value.is_empty() && value != "{}" && value != "null")
        {
            self.tool_calls.push(format!("{name} {arguments}"));
        } else {
            self.tool_calls.push(name.to_string());
        }
    }

    fn apply_tool_result(&mut self, message: &DisplayMessage) {
        let is_error = message
            .payload
            .get("is_error")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let tool_name = message
            .payload
            .get("tool_name")
            .or_else(|| message.payload.get("name"))
            .and_then(Value::as_str)
            .filter(|name| !name.trim().is_empty());
        let content = message
            .payload
            .get("user_content")
            .or_else(|| message.payload.get("content"));
        if tool_name.is_some_and(is_task_tool_name) {
            if let Some(tasks) = content.and_then(task_panel_items_from_value) {
                self.tasks = tasks;
            }
        }
        let preview = content
            .map(|value| value_preview_for_status(value, is_error))
            .or_else(|| message.preview.clone());
        if let Some(preview) = preview {
            let display =
                tool_name.map_or_else(|| preview.clone(), |name| format!("{name} {preview}"));
            if is_error {
                self.tool_calls.push(format!("result:error:{display}"));
            } else {
                self.tool_calls.push(format!("result:{display}"));
            }
        }
    }

    fn apply_task_snapshot(&mut self, message: &DisplayMessage) {
        self.tasks = task_snapshot_items(&message.payload)
            .map(|tasks| {
                tasks
                    .iter()
                    .filter_map(task_panel_item_from_value)
                    .collect()
            })
            .unwrap_or_default();
    }

    fn apply_steering(&mut self, message: &DisplayMessage) {
        let text = message
            .payload
            .get("text")
            .or_else(|| message.payload.get("prompt"))
            .or_else(|| message.payload.get("message"))
            .and_then(Value::as_str)
            .or(message.preview.as_deref())
            .unwrap_or("steering update");
        let prefix = if matches!(message.kind, DisplayMessageKind::SteeringReceived) {
            "received"
        } else {
            "submitted"
        };
        self.steering.push(format!("{prefix}:{text}"));
    }

    /// Render a deterministic text snapshot.
    #[must_use]
    pub fn render_text(&self) -> String {
        let mut output = String::new();
        output.push_str("Starweaver CLI TUI snapshot\n");
        let _ = writeln!(output, "session_id={}", self.session_id);
        let _ = writeln!(output, "messages={}", self.messages);
        let _ = writeln!(output, "pending_approvals={}", self.pending_approvals);
        let _ = writeln!(output, "pending_deferred={}", self.pending_deferred);
        if let Some(status) = self.terminal_status.as_deref() {
            let _ = writeln!(output, "terminal_status={status}");
        }
        if !self.assistant_text.trim().is_empty() {
            output.push_str("\nAssistant\n");
            output.push_str(self.assistant_text.trim());
            output.push('\n');
        }
        if !self.tool_calls.is_empty() {
            output.push_str("\nTools\n");
            for tool in &self.tool_calls {
                output.push_str("- ");
                output.push_str(tool);
                output.push('\n');
            }
        }
        if !self.steering.is_empty() {
            output.push_str("\nSteering\n");
            for item in &self.steering {
                output.push_str("- ");
                output.push_str(item);
                output.push('\n');
            }
        }
        output
    }
}

#[allow(clippy::too_many_lines)]
fn display_message_to_stream_record(
    message: &DisplayMessage,
    sequence: usize,
) -> Option<AgentStreamRecord> {
    let event = match message.kind {
        DisplayMessageKind::AssistantTextDelta => {
            let delta = message_delta(message)?;
            let part_kind = message
                .payload
                .get("part_kind")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    if message
                        .metadata
                        .get("reasoning")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                    {
                        "thinking"
                    } else {
                        "text"
                    }
                });
            let delta = if part_kind == "thinking" {
                starweaver_model::StreamDelta::Thinking { text: delta }
            } else {
                starweaver_model::StreamDelta::Text { text: delta }
            };
            AgentStreamEvent::ModelStream {
                step: 0,
                event: starweaver_runtime::ModelResponseStreamEvent::PartDelta(
                    starweaver_model::PartDelta { index: 0, delta },
                ),
            }
        }
        DisplayMessageKind::ToolCallStart | DisplayMessageKind::ToolCallDelta => {
            let name = message
                .payload
                .get("tool_name")
                .or_else(|| message.payload.get("name"))
                .and_then(Value::as_str)
                .or(message.preview.as_deref())
                .unwrap_or("tool")
                .to_string();
            let arguments = message
                .payload
                .get("arguments")
                .cloned()
                .or_else(|| string_payload(message, "delta").map(Value::String))
                .or_else(|| string_payload(message, "arguments_delta").map(Value::String))
                .unwrap_or_else(|| serde_json::json!({}));
            let tool_call_id = message
                .payload
                .get("tool_call_id")
                .or_else(|| message.payload.get("id"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            AgentStreamEvent::ToolCall {
                step: 0,
                call: ToolCallPart {
                    id: tool_call_id,
                    name,
                    arguments: ToolArguments::from_provider_value(&arguments),
                },
            }
        }
        DisplayMessageKind::ToolResult => {
            let name = message
                .payload
                .get("tool_name")
                .or_else(|| message.payload.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("tool")
                .to_string();
            let content = message
                .payload
                .get("user_content")
                .or_else(|| message.payload.get("content"))
                .cloned()
                .or_else(|| message.preview.clone().map(Value::String))
                .unwrap_or(Value::Null);
            let tool_call_id = message
                .payload
                .get("tool_call_id")
                .or_else(|| message.payload.get("id"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let mut tool_return = ToolReturnPart::new(tool_call_id, name, content);
            tool_return.is_error = message
                .payload
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            tool_return.metadata.clone_from(&message.metadata);
            AgentStreamEvent::ToolReturn {
                step: 0,
                tool_return,
            }
        }
        DisplayMessageKind::ToolsUnavailable => AgentStreamEvent::Custom {
            event: starweaver_context::AgentEvent::new(
                "tools_unavailable",
                message.payload.clone(),
            ),
        },
        DisplayMessageKind::ToolSearchLoaded => AgentStreamEvent::Custom {
            event: starweaver_context::AgentEvent::new(
                "tool_search_loaded",
                message.payload.clone(),
            ),
        },
        DisplayMessageKind::ToolSearchInitialized => AgentStreamEvent::Custom {
            event: starweaver_context::AgentEvent::new(
                "tool_search_initialized",
                message.payload.clone(),
            ),
        },
        DisplayMessageKind::ToolSearchRefreshed => AgentStreamEvent::Custom {
            event: starweaver_context::AgentEvent::new(
                "tool_search_refreshed",
                message.payload.clone(),
            ),
        },
        DisplayMessageKind::ToolSearchInvalidated => AgentStreamEvent::Custom {
            event: starweaver_context::AgentEvent::new(
                "tool_search_invalidated",
                message.payload.clone(),
            ),
        },
        DisplayMessageKind::ToolSearchFailed => AgentStreamEvent::Custom {
            event: starweaver_context::AgentEvent::new(
                "tool_search_failed",
                message.payload.clone(),
            ),
        },
        DisplayMessageKind::ToolSearchNoMatch => AgentStreamEvent::Custom {
            event: starweaver_context::AgentEvent::new(
                "tool_search_no_match",
                message.payload.clone(),
            ),
        },
        DisplayMessageKind::ToolsetInitialized => AgentStreamEvent::Custom {
            event: starweaver_context::AgentEvent::new(
                "toolset_initialized",
                message.payload.clone(),
            ),
        },
        DisplayMessageKind::ToolsetUnavailable => AgentStreamEvent::Custom {
            event: starweaver_context::AgentEvent::new(
                "toolset_unavailable",
                message.payload.clone(),
            ),
        },
        DisplayMessageKind::ToolsetFailed => AgentStreamEvent::Custom {
            event: starweaver_context::AgentEvent::new("toolset_failed", message.payload.clone()),
        },
        DisplayMessageKind::ToolsetRefreshed => AgentStreamEvent::Custom {
            event: starweaver_context::AgentEvent::new(
                "toolset_refreshed",
                message.payload.clone(),
            ),
        },
        DisplayMessageKind::ToolsetClosed => AgentStreamEvent::Custom {
            event: starweaver_context::AgentEvent::new("toolset_closed", message.payload.clone()),
        },
        DisplayMessageKind::ApprovalRequested => AgentStreamEvent::Custom {
            event: starweaver_context::AgentEvent::new(
                "approval_requested",
                message.payload.clone(),
            ),
        },
        DisplayMessageKind::ApprovalResolved => AgentStreamEvent::Custom {
            event: starweaver_context::AgentEvent::new(
                "approval_resolved",
                message.payload.clone(),
            ),
        },
        DisplayMessageKind::HitlResolved => AgentStreamEvent::Custom {
            event: starweaver_context::AgentEvent::new("hitl_resolved", message.payload.clone()),
        },
        DisplayMessageKind::HitlDiagnostic => AgentStreamEvent::Custom {
            event: starweaver_context::AgentEvent::new(
                "hitl_decision_diagnostic",
                message.payload.clone(),
            ),
        },
        DisplayMessageKind::SkillsScanned => AgentStreamEvent::Custom {
            event: starweaver_context::AgentEvent::new("skills_scanned", message.payload.clone()),
        },
        DisplayMessageKind::SkillActivated => AgentStreamEvent::Custom {
            event: starweaver_context::AgentEvent::new("skill_activated", message.payload.clone()),
        },
        DisplayMessageKind::SkillsReloaded => AgentStreamEvent::Custom {
            event: starweaver_context::AgentEvent::new("skills_reloaded", message.payload.clone()),
        },
        DisplayMessageKind::SubagentStarted => AgentStreamEvent::Custom {
            event: starweaver_context::AgentEvent::new("subagent_started", message.payload.clone()),
        },
        DisplayMessageKind::SubagentCompleted => AgentStreamEvent::Custom {
            event: starweaver_context::AgentEvent::new(
                "subagent_completed",
                message.payload.clone(),
            ),
        },
        DisplayMessageKind::SubagentFailed => AgentStreamEvent::Custom {
            event: starweaver_context::AgentEvent::new("subagent_failed", message.payload.clone()),
        },
        DisplayMessageKind::SteeringSubmitted => AgentStreamEvent::Custom {
            event: starweaver_context::AgentEvent::new(
                "steering_submitted",
                message.payload.clone(),
            ),
        },
        DisplayMessageKind::SteeringReceived => AgentStreamEvent::Custom {
            event: starweaver_context::AgentEvent::new(
                "steering_received",
                message.payload.clone(),
            ),
        },
        DisplayMessageKind::TaskSnapshot => AgentStreamEvent::Custom {
            event: starweaver_context::AgentEvent::new(
                TASK_SNAPSHOT_EVENT_KIND,
                message.payload.clone(),
            ),
        },
        DisplayMessageKind::TaskEvent => AgentStreamEvent::Custom {
            event: starweaver_context::AgentEvent::new(
                original_display_event_kind(message, "task_event"),
                message.payload.clone(),
            ),
        },
        DisplayMessageKind::NoteEvent => AgentStreamEvent::Custom {
            event: starweaver_context::AgentEvent::new(
                original_display_event_kind(message, "note_event"),
                message.payload.clone(),
            ),
        },
        DisplayMessageKind::FileEvent => AgentStreamEvent::Custom {
            event: starweaver_context::AgentEvent::new(
                original_display_event_kind(message, "file_event"),
                message.payload.clone(),
            ),
        },
        DisplayMessageKind::MediaEvent => AgentStreamEvent::Custom {
            event: starweaver_context::AgentEvent::new(
                original_display_event_kind(message, "media_event"),
                message.payload.clone(),
            ),
        },
        DisplayMessageKind::HostEvent => AgentStreamEvent::Custom {
            event: starweaver_context::AgentEvent::new(
                original_display_event_kind(message, "host_event"),
                message.payload.clone(),
            ),
        },
        DisplayMessageKind::RunCompleted => AgentStreamEvent::RunComplete {
            run_id: message.run_id.clone(),
            output: message
                .payload
                .get("output")
                .and_then(Value::as_str)
                .or(message.preview.as_deref())
                .unwrap_or_default()
                .to_string(),
        },
        DisplayMessageKind::RunFailed => AgentStreamEvent::RunFailed {
            run_id: message.run_id.clone(),
            error_kind: message
                .payload
                .get("error_kind")
                .and_then(Value::as_str)
                .unwrap_or("error")
                .to_string(),
            message: message
                .payload
                .get("error")
                .and_then(Value::as_str)
                .or(message.preview.as_deref())
                .unwrap_or("run failed")
                .to_string(),
        },
        _ => return None,
    };
    Some(AgentStreamRecord::new(sequence, event))
}

fn message_delta(message: &DisplayMessage) -> Option<String> {
    message
        .payload
        .get("delta")
        .or_else(|| message.payload.get("text"))
        .and_then(Value::as_str)
        .or(message.preview.as_deref())
        .map(ToString::to_string)
}

fn task_snapshot_items(value: &Value) -> Option<&Vec<Value>> {
    let payload = value.get("payload").unwrap_or(value);
    payload
        .get("tasks")
        .and_then(Value::as_array)
        .or_else(|| payload.as_array())
        .or_else(|| {
            value
                .get("tasks")
                .and_then(Value::as_array)
                .or_else(|| value.as_array())
        })
}

fn task_panel_items_from_value(value: &Value) -> Option<Vec<TaskPanelItem>> {
    if let Some(items) = task_snapshot_items(value) {
        return Some(
            items
                .iter()
                .filter_map(task_panel_item_from_value)
                .collect(),
        );
    }
    let payload = value.get("payload").unwrap_or(value);
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

fn is_task_tool_name(name: &str) -> bool {
    matches!(
        name,
        "task_create" | "task_get" | "task_update" | "task_list"
    )
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

fn append_blockquote_text(target: &mut String, text: &str) {
    if !target.is_empty() && !target.ends_with('\n') {
        target.push('\n');
    }
    for line in text.lines() {
        target.push_str("> ");
        target.push_str(line);
        target.push('\n');
    }
}

fn string_payload(message: &DisplayMessage, key: &str) -> Option<String> {
    message
        .payload
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn original_display_event_kind(message: &DisplayMessage, fallback: &str) -> String {
    message
        .metadata
        .get("starweaver_event_kind")
        .and_then(Value::as_str)
        .filter(|kind| !kind.trim().is_empty())
        .unwrap_or(fallback)
        .to_string()
}

fn value_preview(value: &Value) -> String {
    value_preview_for_status(value, false)
}

fn value_preview_for_status(value: &Value, is_error: bool) -> String {
    let text = match value {
        Value::String(value) => value.clone(),
        other => other.to_string(),
    };
    let compact = text.replace('\n', " ");
    if is_error {
        compact
    } else {
        compact.chars().take(80).collect()
    }
}
