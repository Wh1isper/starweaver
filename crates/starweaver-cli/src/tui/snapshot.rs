//! Terminal UI rendering built from display messages.

use std::fmt::Write as _;

use serde::Serialize;
use serde_json::Value;
use starweaver_session::{ApprovalRecord, DeferredToolRecord};
use starweaver_stream::{DisplayMessage, DisplayMessageKind};

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
    /// Pending approval count.
    pub pending_approvals: usize,
    /// Pending deferred tool count.
    pub pending_deferred: usize,
    /// Terminal status if seen.
    pub terminal_status: Option<String>,
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
        for message in messages {
            snapshot.apply_message(&message);
        }
        snapshot
    }

    /// Apply one display message to the retained view snapshot.
    pub fn apply_message(&mut self, message: &DisplayMessage) {
        match message.kind {
            DisplayMessageKind::AssistantTextDelta => {
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
            DisplayMessageKind::ToolCallStart | DisplayMessageKind::ToolCallDelta => {
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
                    .or_else(|| {
                        message
                            .payload
                            .get("delta")
                            .and_then(Value::as_str)
                            .map(ToString::to_string)
                    })
                    .or_else(|| {
                        message
                            .payload
                            .get("arguments_delta")
                            .and_then(Value::as_str)
                            .map(ToString::to_string)
                    });
                if let Some(arguments) =
                    arguments.filter(|value| !value.is_empty() && value != "{}" && value != "null")
                {
                    self.tool_calls.push(format!("{name} {arguments}"));
                } else {
                    self.tool_calls.push(name.to_string());
                }
            }
            DisplayMessageKind::ToolResult => {
                let preview = message
                    .payload
                    .get("content")
                    .map(value_preview)
                    .or_else(|| message.preview.clone());
                if let Some(preview) = preview {
                    if message
                        .payload
                        .get("is_error")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                    {
                        self.tool_calls.push(format!("result:error:{preview}"));
                    } else {
                        self.tool_calls.push(format!("result:{preview}"));
                    }
                }
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
        output
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

fn value_preview(value: &Value) -> String {
    let text = match value {
        Value::String(value) => value.clone(),
        other => other.to_string(),
    };
    text.replace('\n', " ").chars().take(80).collect()
}
