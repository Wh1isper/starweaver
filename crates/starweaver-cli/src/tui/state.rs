use std::{env, path::Path, time::Instant};

use starweaver_runtime::{AgentStreamEvent, AgentStreamRecord, ModelResponseStreamEvent};

use super::{
    render::{snapshot_interactive_lines, value_preview},
    snapshot::TuiSnapshot,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum RunMode {
    Act,
    Plan,
}

impl RunMode {
    pub(super) const fn label(&self) -> &'static str {
        match self {
            Self::Act => "ACT",
            Self::Plan => "PLAN",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum FooterMode {
    Context,
    Shortcuts,
}

impl FooterMode {
    pub(super) const fn is_shortcuts(&self) -> bool {
        matches!(self, Self::Shortcuts)
    }

    pub(super) fn toggle_shortcuts(&mut self) {
        *self = match self {
            Self::Context => Self::Shortcuts,
            Self::Shortcuts => Self::Context,
        };
    }
}

/// Interactive terminal UI state.
#[derive(Clone, Debug)]
pub struct InteractiveTuiState {
    /// Resolved config directory.
    #[allow(dead_code)]
    pub config_dir: String,
    /// Workspace directory displayed in the startup card.
    pub workspace_dir: String,
    /// Current session id, when one exists.
    pub session_id: Option<String>,
    /// Main scrollback body.
    pub body: Vec<String>,
    /// Status line.
    pub status: String,
    /// Editable prompt input.
    pub input: String,
    /// Active profile label.
    pub profile: String,
    /// Active model label.
    pub model: String,
    /// Current runtime phase.
    pub phase: String,
    /// True while a background run is active.
    pub running: bool,
    /// Scrollback offset from bottom.
    pub scroll_offset: usize,
    pub(super) run_mode: RunMode,
    pub(super) history: Vec<String>,
    pub(super) history_index: Option<usize>,
    pub(super) history_draft: String,
    streaming_part: Option<usize>,
    streaming_text_seen: bool,
    pub(super) last_ctrl_c: Option<Instant>,
    pub(super) cancel_requested: bool,
    pub(super) footer_mode: FooterMode,
}

impl InteractiveTuiState {
    /// Create an empty welcome state.
    #[must_use]
    pub fn welcome(config_dir: &Path) -> Self {
        Self {
            config_dir: config_dir.display().to_string(),
            workspace_dir: env::current_dir()
                .map_or_else(|_| ".".to_string(), |path| path.display().to_string()),
            session_id: None,
            body: Vec::new(),
            status: "IDLE".to_string(),
            input: String::new(),
            profile: "general".to_string(),
            model: "local_echo".to_string(),
            phase: "ready".to_string(),
            running: false,
            scroll_offset: 0,
            run_mode: RunMode::Act,
            history: Vec::new(),
            history_index: None,
            history_draft: String::new(),
            streaming_part: None,
            streaming_text_seen: false,
            last_ctrl_c: None,
            cancel_requested: false,
            footer_mode: FooterMode::Context,
        }
    }

    /// Set active profile and model labels.
    pub fn set_profile(&mut self, profile: impl Into<String>, model: impl Into<String>) {
        self.profile = profile.into();
        self.model = model.into();
    }

    /// Replace body with a persisted snapshot.
    pub fn set_snapshot(&mut self, snapshot: &TuiSnapshot) {
        self.session_id = Some(snapshot.session_id.clone());
        self.body = snapshot_interactive_lines(snapshot);
        self.footer_mode = FooterMode::Context;
        self.status = snapshot
            .terminal_status
            .clone()
            .unwrap_or_else(|| "IDLE".to_string())
            .to_ascii_uppercase();
        self.phase = "replay".to_string();
    }

    /// Begin rendering one submitted prompt.
    pub fn begin_run(&mut self, prompt: &str) {
        self.running = true;
        self.cancel_requested = false;
        self.status = "RUNNING".to_string();
        self.phase = "queued".to_string();
        self.streaming_part = None;
        self.streaming_text_seen = false;
        self.footer_mode = FooterMode::Context;
        self.scroll_offset = 0;
        self.body.push(String::new());
        self.body.push(format!("User: {prompt}"));
        self.body.push(String::new());
        self.body.push("Assistant:".to_string());
    }

    /// Mark a run finished with durable ids.
    pub fn finish_run(&mut self, session_id: Option<String>) {
        if let Some(session_id) = session_id {
            self.session_id = Some(session_id);
        }
        self.running = false;
        self.cancel_requested = false;
        self.status = "IDLE".to_string();
        self.phase = "completed".to_string();
        self.streaming_part = None;
    }

    /// Mark a run failed.
    pub fn fail_run(&mut self, error: &str) {
        self.running = false;
        self.cancel_requested = false;
        self.status = "ERROR".to_string();
        self.phase = "failed".to_string();
        self.streaming_part = None;
        self.body.push(format!("Error: {error}"));
    }

    /// Apply a live runtime stream event to the view state.
    pub fn apply_stream_record(&mut self, record: &AgentStreamRecord) {
        match &record.event {
            AgentStreamEvent::RunStart { .. } => {
                self.status = "RUNNING".to_string();
                self.phase = "started".to_string();
            }
            AgentStreamEvent::NodeStart { node, .. } => {
                self.phase = format!("node:{node:?}").to_ascii_lowercase();
            }
            AgentStreamEvent::ModelRequest { .. } => {
                self.phase = "thinking".to_string();
            }
            AgentStreamEvent::ModelStream { event, .. } => self.apply_model_stream_event(event),
            AgentStreamEvent::ModelResponse { response, .. } => {
                self.phase = "response".to_string();
                for part in &response.parts {
                    match part {
                        starweaver_model::ModelResponsePart::Text { text }
                            if !self.streaming_text_seen =>
                        {
                            self.push_text_lines(text);
                            self.streaming_text_seen = true;
                        }
                        starweaver_model::ModelResponsePart::Thinking { text, .. } => {
                            self.body.push(format!("Thinking: {text}"));
                        }
                        starweaver_model::ModelResponsePart::ToolCall(call) => {
                            self.body.push(format!("Tool call: {}", call.name));
                            self.phase = "tools".to_string();
                        }
                        _ => {}
                    }
                }
            }
            AgentStreamEvent::ToolCall { call, .. } => {
                self.phase = "tools".to_string();
                self.body.push(format!("Tool call: {}", call.name));
            }
            AgentStreamEvent::ToolReturn { tool_return, .. } => {
                self.phase = "tools".to_string();
                self.body.push(format!(
                    "Tool result: {} {}",
                    tool_return.name,
                    value_preview(&tool_return.content)
                ));
            }
            AgentStreamEvent::OutputRetry { retries, .. } => {
                self.phase = "retry".to_string();
                self.body.push(format!("Output retry: {retries}"));
            }
            AgentStreamEvent::Suspended { reason, .. } => {
                self.status = "WAITING".to_string();
                self.phase = "suspended".to_string();
                self.body.push(format!("Suspended: {reason}"));
            }
            AgentStreamEvent::Checkpoint { node, .. } => {
                self.phase = format!("checkpoint:{node:?}").to_ascii_lowercase();
            }
            AgentStreamEvent::Custom { event } => {
                self.phase.clone_from(&event.kind);
            }
            AgentStreamEvent::RunComplete { output, .. } => {
                self.phase = "completed".to_string();
                if !self.streaming_text_seen && !output.trim().is_empty() {
                    self.push_text_lines(output);
                    self.streaming_text_seen = true;
                }
            }
            AgentStreamEvent::NodeComplete { .. } => {}
        }
        self.scroll_offset = 0;
    }

    fn apply_model_stream_event(&mut self, event: &ModelResponseStreamEvent) {
        match event {
            ModelResponseStreamEvent::PartStart(part) => {
                self.streaming_part = Some(part.index);
                self.phase = if part.part_kind.contains("thinking") {
                    "thinking".to_string()
                } else {
                    "streaming".to_string()
                };
                if part.part_kind.contains("thinking") {
                    self.body.push("Thinking:".to_string());
                }
            }
            ModelResponseStreamEvent::PartDelta(delta) => {
                self.phase = "streaming".to_string();
                self.append_stream_delta(&delta.delta);
                self.streaming_text_seen = true;
            }
            ModelResponseStreamEvent::PartEnd(part) => {
                if self.streaming_part == Some(part.index) {
                    self.streaming_part = None;
                }
            }
            ModelResponseStreamEvent::FinalResult(response) => {
                self.phase = "finalizing".to_string();
                if !self.streaming_text_seen {
                    let text = response.text_output();
                    if !text.trim().is_empty() {
                        self.push_text_lines(&text);
                        self.streaming_text_seen = true;
                    }
                }
            }
        }
    }

    fn append_stream_delta(&mut self, delta: &str) {
        if self.body.is_empty() {
            self.body.push(String::new());
        }
        for segment in delta.split_inclusive('\n') {
            if segment.ends_with('\n') {
                let trimmed = segment.trim_end_matches('\n').trim_end_matches('\r');
                if !trimmed.is_empty() {
                    if let Some(last) = self.body.last_mut() {
                        last.push_str(trimmed);
                    }
                }
                self.body.push(String::new());
            } else if let Some(last) = self.body.last_mut() {
                last.push_str(segment);
            }
        }
    }

    fn push_text_lines(&mut self, text: &str) {
        for line in text.lines() {
            self.body.push(line.to_string());
        }
    }

    pub(super) fn push_history(&mut self, prompt: String) {
        if self.history.last() != Some(&prompt) {
            self.history.push(prompt);
        }
        self.history_index = None;
        self.history_draft.clear();
    }

    pub(super) fn previous_history(&mut self) {
        if self.history.is_empty() {
            return;
        }
        if self.history_index.is_none() {
            self.history_draft = self.input.clone();
            self.history_index = Some(self.history.len().saturating_sub(1));
        } else if let Some(index) = self.history_index.as_mut() {
            *index = index.saturating_sub(1);
        }
        if let Some(index) = self.history_index {
            self.input = self.history[index].clone();
        }
    }

    pub(super) fn next_history(&mut self) {
        let Some(index) = self.history_index else {
            return;
        };
        if index + 1 >= self.history.len() {
            self.history_index = None;
            self.input = self.history_draft.clone();
            self.history_draft.clear();
        } else {
            let next = index + 1;
            self.history_index = Some(next);
            self.input = self.history[next].clone();
        }
    }

    pub(super) fn request_cancel(&mut self) {
        self.cancel_requested = true;
        self.status = "INTERRUPT".to_string();
        self.phase = "cancel requested".to_string();
        self.body
            .push("Interrupt requested. Current run will finish in the background.".to_string());
    }

    pub(super) fn show_run_active_hint(&mut self) {
        self.status = "RUNNING".to_string();
        self.phase = "run active; press Ctrl-C to interrupt".to_string();
    }
}
