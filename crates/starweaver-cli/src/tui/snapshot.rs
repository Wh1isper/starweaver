//! Terminal UI rendering built from display messages.

use std::fmt::Write as _;

use serde::Serialize;
use serde_json::Value;
use starweaver_model::{ToolArguments, ToolCallPart, ToolReturnPart};
use starweaver_runtime::{AgentStreamEvent, AgentStreamRecord};
use starweaver_session::{ApprovalRecord, DeferredToolRecord};
use starweaver_stream::{DisplayMessage, DisplayMessageKind};

use super::state::display_lines_for_stream_record;

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
        let preview = message
            .payload
            .get("user_content")
            .or_else(|| message.payload.get("content"))
            .map(|value| value_preview_for_status(value, is_error))
            .or_else(|| message.preview.clone());
        if let Some(preview) = preview {
            let display = message
                .payload
                .get("tool_name")
                .or_else(|| message.payload.get("name"))
                .and_then(Value::as_str)
                .filter(|name| !name.trim().is_empty())
                .map_or_else(|| preview.clone(), |name| format!("{name} {preview}"));
            if is_error {
                self.tool_calls.push(format!("result:error:{display}"));
            } else {
                self.tool_calls.push(format!("result:{display}"));
            }
        }
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
