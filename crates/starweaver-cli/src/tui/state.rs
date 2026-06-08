use std::{
    collections::{BTreeMap, HashMap, HashSet},
    env,
    path::Path,
    process::Command,
    time::Instant,
};

use serde::{Deserialize, Serialize};

use starweaver_core::Usage;
use starweaver_model::{PartDelta, StreamDelta};
use starweaver_runtime::{AgentStreamEvent, AgentStreamRecord, ModelResponseStreamEvent};

const DEFAULT_CONTEXT_WINDOW_TOKENS: u64 = 200_000;
const MAX_STEERING_ITEMS: usize = 5;
const SHELL_OUTPUT_MAX_LINES: usize = 200;

use super::{
    markdown::ASSISTANT_CONTENT_PREFIX,
    render::{snapshot_interactive_lines, value_preview},
    snapshot::TuiSnapshot,
};

/// Config-defined slash command prompt.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SlashCommandDefinition {
    /// Canonical command name without a leading slash.
    pub name: String,
    /// Prompt submitted when the command is invoked.
    pub prompt: String,
    /// Optional mode hint such as `act` or `plan`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// Human-readable description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Additional aliases without a leading slash.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
}

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
}

impl FooterMode {
    #[cfg(test)]
    pub(super) const fn is_help() -> bool {
        false
    }
}

#[allow(dead_code)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum SteeringStatus {
    Pending,
    Acked,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SteeringItem {
    pub(super) id: String,
    pub(super) text: String,
    pub(super) status: SteeringStatus,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SteeringSubmission {
    pub id: String,
    pub text: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StreamingPartKind {
    Text,
    Thinking,
    ToolCall,
    Other,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ModelChoice {
    /// Profile id used by `--profile` and `/model`.
    pub profile: String,
    /// Optional human label.
    pub label: Option<String>,
    /// Provider model id.
    pub model_id: String,
    /// Model settings preset name, when configured.
    pub model_settings: Option<String>,
    /// Model config preset name, when configured.
    pub model_cfg: Option<String>,
    /// Context window in tokens for this profile, when known.
    pub context_window: Option<u64>,
    /// Profile source kind.
    pub source: String,
}

impl ModelChoice {
    pub(super) fn display_name(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.profile)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct StreamingToolCallState {
    name: Option<String>,
    arguments: String,
    line_index: Option<usize>,
}

/// Interactive terminal UI state.
#[allow(clippy::struct_excessive_bools)]
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
    /// Short-lived composer status for paste, media attach, and steering actions.
    pub(super) input_status: Option<String>,
    /// Image paths pasted into the fixed composer.
    pub(super) pasted_images: Vec<String>,
    pub(super) run_mode: RunMode,
    pub(super) history: Vec<String>,
    pub(super) history_index: Option<usize>,
    pub(super) history_draft: String,
    streaming_parts: HashMap<usize, StreamingPartKind>,
    streaming_text_seen: bool,
    streaming_reasoning_seen: bool,
    streaming_tool_calls: HashMap<usize, StreamingToolCallState>,
    visible_tool_calls: HashSet<String>,
    model_choices: Vec<ModelChoice>,
    model_picker_open: bool,
    model_picker_index: usize,
    pub(super) last_ctrl_c: Option<Instant>,
    pub(super) cancel_requested: bool,
    pub(super) footer_mode: FooterMode,
    pub(super) goal_task: Option<String>,
    pub(super) goal_active: bool,
    pub(super) goal_iteration: usize,
    pub(super) goal_max_iterations: usize,
    pub(super) context_tokens: Option<u64>,
    pub(super) context_window: Option<u64>,
    pub(super) steering_items: Vec<SteeringItem>,
    next_steering_id: u64,
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
            scroll_offset: usize::MAX,
            input_status: None,
            pasted_images: Vec::new(),
            run_mode: RunMode::Act,
            history: Vec::new(),
            history_index: None,
            history_draft: String::new(),
            streaming_parts: HashMap::new(),
            streaming_text_seen: false,
            streaming_reasoning_seen: false,
            streaming_tool_calls: HashMap::new(),
            visible_tool_calls: HashSet::new(),
            model_choices: Vec::new(),
            model_picker_open: false,
            model_picker_index: 0,
            last_ctrl_c: None,
            cancel_requested: false,
            footer_mode: FooterMode::Context,
            goal_task: None,
            goal_active: false,
            goal_iteration: 0,
            goal_max_iterations: 10,
            context_tokens: None,
            context_window: Some(DEFAULT_CONTEXT_WINDOW_TOKENS),
            steering_items: Vec::new(),
            next_steering_id: 0,
        }
    }

    /// Set active profile and model labels.
    pub fn set_profile(&mut self, profile: impl Into<String>, model: impl Into<String>) {
        self.profile = profile.into();
        self.model = model.into();
    }

    /// Set the context window shown by the footer and cost summary.
    pub fn set_context_window(&mut self, context_window: Option<u64>) {
        self.context_window = context_window.or(Some(DEFAULT_CONTEXT_WINDOW_TOKENS));
    }

    /// Set model choices shown by `/model`.
    pub fn set_model_choices(&mut self, choices: Vec<ModelChoice>) {
        self.model_choices = choices;
        self.sync_model_picker_index_to_current();
    }

    /// Return configured model choices.
    pub fn model_choices(&self) -> &[ModelChoice] {
        &self.model_choices
    }

    pub(super) const fn model_picker_visible(&self) -> bool {
        self.model_picker_open
    }

    pub(super) const fn model_picker_index(&self) -> usize {
        self.model_picker_index
    }

    pub(super) fn open_model_picker(&mut self) {
        if self.running {
            self.body.push(
                "[SYS] Model selection is available after the current run finishes.".to_string(),
            );
            self.input_status = Some("model blocked".to_string());
            return;
        }
        self.input.clear();
        self.pasted_images.clear();
        self.footer_mode = FooterMode::Context;
        self.model_picker_open = true;
        self.sync_model_picker_index_to_current();
        self.input_status = Some("model picker".to_string());
    }

    pub(super) fn close_model_picker(&mut self) {
        self.model_picker_open = false;
        self.input_status = Some("model picker closed".to_string());
    }

    pub(super) fn move_model_picker_selection(&mut self, delta: isize) {
        let len = self.model_choices.len();
        if len == 0 {
            self.model_picker_index = 0;
            return;
        }
        let current = self.model_picker_index.min(len.saturating_sub(1));
        let steps = delta.unsigned_abs() % len;
        self.model_picker_index = if delta.is_negative() {
            (current + len - steps) % len
        } else {
            (current + steps) % len
        };
        self.input_status = Some("model picker".to_string());
    }

    pub(super) fn select_model_picker_choice(&mut self) {
        let Some(choice) = self.model_choices.get(self.model_picker_index).cloned() else {
            self.close_model_picker();
            return;
        };
        self.apply_model_choice(&choice);
        self.model_picker_open = false;
        self.input_status = Some("model selected".to_string());
    }

    fn sync_model_picker_index_to_current(&mut self) {
        self.model_picker_index = self
            .model_choices
            .iter()
            .position(|choice| choice.profile == self.profile)
            .unwrap_or(0)
            .min(self.model_choices.len().saturating_sub(1));
    }

    /// Replace body with a persisted snapshot.
    pub fn set_snapshot(&mut self, snapshot: &TuiSnapshot) {
        self.session_id = Some(snapshot.session_id.clone());
        self.body = snapshot_interactive_lines(snapshot);
        self.footer_mode = FooterMode::Context;
        self.input_status = None;
        self.pasted_images.clear();
        self.status = snapshot
            .terminal_status
            .clone()
            .unwrap_or_else(|| "IDLE".to_string())
            .to_ascii_uppercase();
        self.model_picker_open = false;
        self.phase = "replay".to_string();
    }

    /// Begin rendering one submitted prompt.
    pub fn begin_run(&mut self, prompt: &str) {
        self.running = true;
        self.cancel_requested = false;
        self.status = "RUNNING".to_string();
        self.phase = "queued".to_string();
        self.streaming_parts.clear();
        self.streaming_text_seen = false;
        self.streaming_reasoning_seen = false;
        self.streaming_tool_calls.clear();
        self.visible_tool_calls.clear();
        self.footer_mode = FooterMode::Context;
        self.model_picker_open = false;
        self.input_status = None;
        self.pasted_images.clear();
        self.scroll_to_bottom();
        self.body.push(String::new());
        self.body.push(format!("User: {prompt}"));
        self.body.push(String::new());
        self.body.push("Assistant:".to_string());
        self.body.push(assistant_content_line(""));
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
        self.streaming_parts.clear();
        self.streaming_tool_calls.clear();
        self.visible_tool_calls.clear();
        self.model_picker_open = false;
    }

    /// Mark a run failed.
    pub fn fail_run(&mut self, error: &str) {
        self.running = false;
        self.cancel_requested = false;
        self.status = "ERROR".to_string();
        self.phase = "failed".to_string();
        self.streaming_parts.clear();
        self.streaming_tool_calls.clear();
        self.visible_tool_calls.clear();
        self.model_picker_open = false;
        self.body.push(format!("Error: {error}"));
    }

    /// Mark a run cancelled by the interactive user.
    pub fn cancel_run(&mut self, reason: &str) {
        self.running = false;
        self.cancel_requested = false;
        self.status = "IDLE".to_string();
        self.phase = "cancelled".to_string();
        self.streaming_parts.clear();
        self.streaming_tool_calls.clear();
        self.visible_tool_calls.clear();
        self.model_picker_open = false;
        self.body.push(format!("Run cancelled: {reason}"));
    }

    /// Apply a live runtime stream event to the view state.
    pub fn apply_stream_record(&mut self, record: &AgentStreamRecord) {
        let was_at_bottom = self.is_at_bottom();
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
                self.add_context_usage(&response.usage);
                self.apply_model_response_parts(&response.parts);
            }
            AgentStreamEvent::ToolCall { call, .. } => {
                self.push_tool_call(call);
            }
            AgentStreamEvent::ToolReturn { tool_return, .. } => {
                self.phase = "tools".to_string();
                let prefix = if tool_return.is_error {
                    "Tool error"
                } else {
                    "Tool result"
                };
                self.body.push(format!(
                    "{prefix}: {} {}",
                    tool_return.name,
                    value_preview(&tool_return.content)
                ));
            }
            AgentStreamEvent::OutputRetry { retries, .. } => {
                self.phase = "retry".to_string();
                self.body.push(format!("Output retry: {retries}"));
            }
            AgentStreamEvent::SteeringGuard { .. } => {
                self.phase = "steering".to_string();
                self.body
                    .push("Steering update pending; continuing run.".to_string());
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
                if event.kind == "steering_received" {
                    let steering_id = event
                        .payload
                        .get("id")
                        .or_else(|| event.payload.get("message_id"))
                        .and_then(serde_json::Value::as_str);
                    let text = event
                        .payload
                        .get("text")
                        .and_then(serde_json::Value::as_str);
                    self.ack_steering_event(steering_id, text);
                }
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
        if was_at_bottom {
            self.scroll_to_bottom();
        }
    }

    fn apply_model_stream_event(&mut self, event: &ModelResponseStreamEvent) {
        match event {
            ModelResponseStreamEvent::PartStart(part) => {
                let kind = streaming_part_kind(&part.part_kind);
                self.streaming_parts.insert(part.index, kind);
                self.phase = match kind {
                    StreamingPartKind::Text => {
                        self.ensure_text_stream_line();
                        "streaming".to_string()
                    }
                    StreamingPartKind::Thinking => "thinking".to_string(),
                    StreamingPartKind::ToolCall => {
                        self.ensure_streaming_tool_call_line(part.index);
                        "tools".to_string()
                    }
                    StreamingPartKind::Other => format!("streaming:{}", part.part_kind),
                };
            }
            ModelResponseStreamEvent::PartDelta(delta) => {
                match self.streaming_kind_for_delta(delta) {
                    StreamingPartKind::Text => {
                        self.phase = "streaming".to_string();
                        self.append_stream_delta(&delta.as_text());
                        self.streaming_text_seen = true;
                    }
                    StreamingPartKind::Thinking => {
                        self.phase = "thinking".to_string();
                        self.append_thinking_delta(&delta.as_text());
                        self.streaming_reasoning_seen = true;
                    }
                    StreamingPartKind::ToolCall => {
                        self.phase = "tools".to_string();
                        self.append_tool_call_delta(delta);
                    }
                    StreamingPartKind::Other => {
                        self.phase = "streaming".to_string();
                    }
                }
            }
            ModelResponseStreamEvent::PartEnd(part) => {
                self.streaming_parts.remove(&part.index);
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
                self.apply_model_response_parts(&response.parts);
            }
        }
    }

    fn append_stream_delta(&mut self, delta: &str) {
        self.ensure_text_stream_line();
        append_delta_segments(&mut self.body, delta, |line| assistant_content_line(line));
    }

    fn append_thinking_delta(&mut self, delta: &str) {
        self.ensure_thinking_blockquote();
        append_delta_segments(&mut self.body, delta, |line| {
            assistant_content_line(format!("> {line}"))
        });
    }

    fn ensure_thinking_blockquote(&mut self) {
        if !self
            .body
            .last()
            .is_some_and(|line| is_thinking_quote_line(line))
        {
            self.body.push(assistant_content_line("> "));
        }
    }

    fn push_thinking_lines(&mut self, text: &str) {
        let mut lines = text.lines().peekable();
        if lines.peek().is_none() {
            self.ensure_thinking_blockquote();
            return;
        }
        for line in lines {
            self.body.push(assistant_content_line(format!("> {line}")));
        }
    }

    fn streaming_kind_for_delta(&self, delta: &PartDelta) -> StreamingPartKind {
        match &delta.delta {
            StreamDelta::Text { .. } => StreamingPartKind::Text,
            StreamDelta::Thinking { .. } => StreamingPartKind::Thinking,
            StreamDelta::ToolCallName { .. } | StreamDelta::ToolCallArguments { .. } => {
                StreamingPartKind::ToolCall
            }
            StreamDelta::NativePayload { .. } | StreamDelta::FileMetadata { .. } => self
                .streaming_parts
                .get(&delta.index)
                .copied()
                .unwrap_or(StreamingPartKind::Other),
        }
    }

    fn push_text_lines(&mut self, text: &str) {
        self.ensure_text_stream_line();
        let mut lines = text.lines().peekable();
        if lines.peek().is_none() {
            return;
        }
        for line in lines {
            self.body.push(assistant_content_line(line));
        }
    }

    fn ensure_text_stream_line(&mut self) {
        if self.body.is_empty()
            || self.body.last().is_some_and(|line| {
                !is_assistant_content_line(line) || is_thinking_quote_line(line)
            })
        {
            self.body.push(assistant_content_line(""));
        }
    }

    fn apply_model_response_parts(&mut self, parts: &[starweaver_model::ModelResponsePart]) {
        for part in parts {
            match part {
                starweaver_model::ModelResponsePart::Text { text } if !self.streaming_text_seen => {
                    self.push_text_lines(text);
                    self.streaming_text_seen = true;
                }
                starweaver_model::ModelResponsePart::Thinking { text, .. }
                    if !self.streaming_reasoning_seen =>
                {
                    self.push_thinking_lines(text);
                    self.streaming_reasoning_seen = true;
                }
                starweaver_model::ModelResponsePart::ToolCall(call) => {
                    self.push_tool_call(call);
                }
                _ => {}
            }
        }
    }

    fn append_tool_call_delta(&mut self, delta: &PartDelta) {
        match &delta.delta {
            StreamDelta::ToolCallName { name } => {
                let state = self.streaming_tool_calls.entry(delta.index).or_default();
                state.name = Some(merge_stream_fragment(state.name.as_deref(), name));
            }
            StreamDelta::ToolCallArguments { arguments_delta } => {
                let state = self.streaming_tool_calls.entry(delta.index).or_default();
                state.arguments.push_str(arguments_delta);
            }
            _ => {}
        }
        self.update_streaming_tool_call_line(delta.index);
    }

    fn ensure_streaming_tool_call_line(&mut self, index: usize) {
        if self
            .streaming_tool_calls
            .get(&index)
            .and_then(|state| state.line_index)
            .is_some()
        {
            return;
        }
        let line_index = self.body.len();
        self.body.push(format_streaming_tool_call_line(
            self.streaming_tool_calls.get(&index),
        ));
        self.streaming_tool_calls
            .entry(index)
            .or_default()
            .line_index = Some(line_index);
    }

    fn update_streaming_tool_call_line(&mut self, index: usize) {
        self.ensure_streaming_tool_call_line(index);
        let line = format_streaming_tool_call_line(self.streaming_tool_calls.get(&index));
        if let Some(line_index) = self
            .streaming_tool_calls
            .get(&index)
            .and_then(|state| state.line_index)
        {
            if let Some(existing) = self.body.get_mut(line_index) {
                *existing = line;
            }
        }
    }

    fn push_tool_call(&mut self, call: &starweaver_model::ToolCallPart) {
        self.phase = "tools".to_string();
        let key = tool_call_visibility_key(call);
        if !self.visible_tool_calls.insert(key) {
            return;
        }
        let line = format_tool_call_line(call);
        if let Some(line_index) = self.matching_streamed_tool_line(call) {
            if let Some(existing) = self.body.get_mut(line_index) {
                *existing = line;
                return;
            }
        }
        self.body.push(line);
    }

    fn matching_streamed_tool_line(&self, call: &starweaver_model::ToolCallPart) -> Option<usize> {
        self.streaming_tool_calls
            .values()
            .find(|state| state.name.as_deref() == Some(call.name.as_str()))
            .and_then(|state| state.line_index)
    }

    pub(super) fn apply_paste(&mut self, text: &str) {
        self.footer_mode = FooterMode::Context;
        let images = pasted_image_paths(text);
        if images.is_empty() {
            self.input.push_str(text);
            self.input_status = Some(format!("pasted {} chars", text.chars().count()));
            return;
        }

        self.pasted_images.extend(images);
        let count = self.pasted_images.len();
        self.input_status = Some(if count == 1 {
            format!(
                "image attached: {}",
                compact_path(&self.pasted_images[0], 42)
            )
        } else {
            format!("images attached: {count}")
        });
    }

    pub(super) fn take_submission_prompt(&mut self) -> Option<String> {
        self.take_prompt(SubmissionKind::Message)
    }

    pub(super) fn take_queued_prompt(&mut self) -> Option<String> {
        self.take_prompt(SubmissionKind::Queued)
    }

    pub(super) fn take_steering_prompt(&mut self) -> Option<SteeringSubmission> {
        self.take_prompt(SubmissionKind::Steering)
            .map(|text| self.record_steering_message(text))
    }

    fn take_prompt(&mut self, kind: SubmissionKind) -> Option<String> {
        let command = self.take_local_command();
        if matches!(command, LocalCommandOutcome::Consumed) {
            return None;
        }
        let prompt = match command {
            LocalCommandOutcome::Submit(prompt) => prompt,
            LocalCommandOutcome::Consumed => unreachable!("handled above"),
            LocalCommandOutcome::None => self.input.trim().to_string(),
        };
        if prompt.is_empty() && self.pasted_images.is_empty() {
            self.input.clear();
            return None;
        }
        let mut output = prompt;
        for image in &self.pasted_images {
            if !output.is_empty() {
                output.push('\n');
            }
            output.push_str("[image: ");
            output.push_str(image);
            output.push(']');
        }
        self.input.clear();
        self.pasted_images.clear();
        match kind {
            SubmissionKind::Message => self.input_status = Some("message sent".to_string()),
            SubmissionKind::Queued => self.input_status = Some("queued".to_string()),
            SubmissionKind::Steering => {
                self.input_status = Some("steer sent".to_string());
            }
        }
        Some(output)
    }

    pub(super) fn clear_composer(&mut self) {
        self.input.clear();
        self.pasted_images.clear();
        self.input_status = None;
        self.history_index = None;
        self.footer_mode = FooterMode::Context;
    }

    pub(super) fn backspace_composer(&mut self) {
        if self.input.pop().is_none() {
            self.remove_last_pasted_image();
        }
    }

    fn remove_last_pasted_image(&mut self) {
        if self.pasted_images.pop().is_some() {
            self.input_status = Some(if self.pasted_images.is_empty() {
                "image detached".to_string()
            } else {
                format!("images attached: {}", self.pasted_images.len())
            });
        }
    }

    pub(super) fn composer_is_empty(&self) -> bool {
        self.input.is_empty() && self.pasted_images.is_empty()
    }

    pub(super) fn composer_has_draft(&self) -> bool {
        !self.input.trim().is_empty() || !self.pasted_images.is_empty()
    }

    pub(super) fn input_mode_label(&self) -> &'static str {
        if self.model_picker_open {
            "MODEL"
        } else if self.running && self.composer_has_draft() {
            "STEER"
        } else if self.running {
            "RUNNING"
        } else {
            self.run_mode.label()
        }
    }

    #[cfg(test)]
    pub(super) fn input_status_text(&self) -> &str {
        self.input_status.as_deref().unwrap_or(&self.phase)
    }

    pub(super) const fn help_panel_visible() -> bool {
        false
    }

    /// Config-defined slash commands are ignored by the simplified TUI command surface.
    pub fn set_custom_commands(&mut self, _commands: BTreeMap<String, SlashCommandDefinition>) {
        self.footer_mode = FooterMode::Context;
    }

    fn take_local_command(&mut self) -> LocalCommandOutcome {
        let input = self.input.trim().to_string();
        if input == "/help" {
            self.input.clear();
            self.pasted_images.clear();
            self.footer_mode = FooterMode::Context;
            self.append_help_to_body();
            self.input_status = Some("help".to_string());
            return LocalCommandOutcome::Consumed;
        }
        if input == "/clear" {
            self.input.clear();
            self.body.clear();
            self.footer_mode = FooterMode::Context;
            self.input_status = Some("cleared".to_string());
            return LocalCommandOutcome::Consumed;
        }
        if input == "/cost" {
            self.input.clear();
            self.pasted_images.clear();
            self.footer_mode = FooterMode::Context;
            self.append_cost_summary();
            self.input_status = Some("cost".to_string());
            return LocalCommandOutcome::Consumed;
        }
        if input == "/model" || input.starts_with("/model ") {
            self.input.clear();
            self.pasted_images.clear();
            self.footer_mode = FooterMode::Context;
            self.handle_model_command(input.strip_prefix("/model").unwrap_or_default().trim());
            if !self.model_picker_open {
                self.input_status = Some("model".to_string());
            }
            return LocalCommandOutcome::Consumed;
        }
        if let Some(command) = input.strip_prefix('!') {
            self.input.clear();
            self.pasted_images.clear();
            self.footer_mode = FooterMode::Context;
            self.run_shell_command(command.trim());
            self.input_status = Some("shell".to_string());
            return LocalCommandOutcome::Consumed;
        }
        if let Some(task) = input.strip_prefix("/goal") {
            self.input.clear();
            let task = task.trim();
            if task.is_empty() {
                self.body
                    .push("[SYS] Usage: /goal <task description>".to_string());
                self.input_status = Some("goal usage".to_string());
                return LocalCommandOutcome::Consumed;
            }
            self.goal_task = Some(task.to_string());
            self.goal_active = true;
            self.goal_iteration = 0;
            self.goal_max_iterations = self.goal_max_iterations.max(1);
            self.body.push(format!(
                "[SYS] [Goal] Starting goal mode ({} max iterations). Ctrl+C to stop.",
                self.goal_max_iterations
            ));
            self.input_status = Some("goal".to_string());
            return LocalCommandOutcome::Submit(task.to_string());
        }
        if input.starts_with('/') {
            self.input.clear();
            self.pasted_images.clear();
            self.footer_mode = FooterMode::Context;
            self.body.push(format!(
                "[SYS] Unknown command: {input}. Available commands: /help, /clear, /cost, /model, /goal, !<command>"
            ));
            self.input_status = Some("unknown command".to_string());
            return LocalCommandOutcome::Consumed;
        }
        LocalCommandOutcome::None
    }

    fn append_help_to_body(&mut self) {
        self.body.extend([
            "Starweaver TUI help".to_string(),
            String::new(),
            "Commands".to_string(),
            "  /help             Show this help".to_string(),
            "  /clear            Clear the transcript".to_string(),
            "  /cost             Show usage and context".to_string(),
            "  /model [profile]  Open or select a model profile".to_string(),
            "  /goal <task>      Run toward a verified goal".to_string(),
            "  !<command>        Run a shell command inline".to_string(),
            String::new(),
            "Shortcuts".to_string(),
            "  Up/Down           Browse prompt history".to_string(),
            "  PageUp/PageDown   Scroll transcript".to_string(),
            "  Mouse wheel       Scroll transcript".to_string(),
            "  Enter             Send message or select model".to_string(),
            "  Tab               Queue a draft while running".to_string(),
            "  Ctrl+C            Interrupt or exit".to_string(),
        ]);
    }

    fn handle_model_command(&mut self, requested: &str) {
        if self.running {
            self.body.push(
                "[SYS] Model selection is available after the current run finishes.".to_string(),
            );
            return;
        }
        if requested.is_empty() {
            self.open_model_picker();
            return;
        }
        let Some(choice) = self
            .model_choices
            .iter()
            .find(|choice| choice.profile == requested || choice.display_name() == requested)
            .cloned()
        else {
            self.body
                .push(format!("[SYS] Unknown model profile: {requested}"));
            self.append_model_choices();
            return;
        };
        self.apply_model_choice(&choice);
    }

    fn apply_model_choice(&mut self, choice: &ModelChoice) {
        self.profile.clone_from(&choice.profile);
        self.model = model_choice_label(choice);
        self.set_context_window(choice.context_window);
        self.sync_model_picker_index_to_current();
        self.body.push(format!(
            "[SYS] Switched model to {} ({})",
            choice.display_name(),
            choice.model_id
        ));
    }

    fn append_model_choices(&mut self) {
        self.body.push("[SYS] Model profiles".to_string());
        self.body
            .push(format!("[SYS] Current: {} ({})", self.profile, self.model));
        if self.model_choices.is_empty() {
            self.body
                .push("[SYS] No model profiles are configured.".to_string());
            return;
        }
        for choice in &self.model_choices {
            let marker = if choice.profile == self.profile {
                "*"
            } else {
                " "
            };
            self.body.push(format!(
                "[SYS] {marker} /model {:<18} {} ({}){}",
                choice.profile,
                choice.display_name(),
                choice.model_id,
                model_choice_config_suffix(choice)
            ));
        }
    }

    fn append_cost_summary(&mut self) {
        self.body.push("[SYS] Cost summary".to_string());
        match self.context_tokens {
            Some(tokens) => self.body.push(format!("[SYS] Context tokens: {tokens}")),
            None => self.body.push("[SYS] Context tokens: 0".to_string()),
        }
        if let Some(window) = self.context_window {
            self.body.push(format!("[SYS] Context window: {window}"));
            self.body.push(format!(
                "[SYS] Context used: {}",
                self.context_percent_label()
            ));
        }
        self.body
            .push("[SYS] Cost data: unavailable in current run".to_string());
    }

    fn run_shell_command(&mut self, command: &str) {
        if command.is_empty() {
            self.body.push(
                "[SYS] Shell command usage: !<command> (example: !git status --short)".to_string(),
            );
            return;
        }
        self.body.push(format!("Shell command: {command}"));
        match Command::new("/bin/bash").arg("-lc").arg(command).output() {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                push_shell_output_lines(&mut self.body, "stdout", &stdout);
                push_shell_output_lines(&mut self.body, "stderr", &stderr);
                let status = output
                    .status
                    .code()
                    .map_or_else(|| "signal".to_string(), |code| code.to_string());
                if output.status.success() {
                    self.body.push(format!("Shell completed: exit {status}"));
                } else {
                    self.body.push(format!("Shell failed: exit {status}"));
                }
            }
            Err(error) => self.body.push(format!("Shell error: {error}")),
        }
    }

    pub(super) fn steering_items(&self) -> &[SteeringItem] {
        &self.steering_items
    }

    pub(crate) fn ack_steering_event(&mut self, id: Option<&str>, text: Option<&str>) {
        let index = if let Some(id) = id {
            self.steering_items
                .iter()
                .position(|item| matches!(item.status, SteeringStatus::Pending) && item.id == id)
        } else {
            text.and_then(|text| {
                self.steering_items.iter().position(|item| {
                    matches!(item.status, SteeringStatus::Pending) && item.text == text
                })
            })
        };
        if let Some(index) = index {
            self.steering_items[index].status = SteeringStatus::Acked;
        }
    }

    fn record_steering_message(&mut self, text: String) -> SteeringSubmission {
        let id = format!("steer_{}", self.next_steering_id);
        self.next_steering_id = self.next_steering_id.saturating_add(1);
        self.steering_items.push(SteeringItem {
            id: id.clone(),
            text: text.clone(),
            status: SteeringStatus::Pending,
        });
        let overflow = self.steering_items.len().saturating_sub(MAX_STEERING_ITEMS);
        if overflow > 0 {
            self.steering_items.drain(0..overflow);
        }
        SteeringSubmission { id, text }
    }

    pub(super) const fn scroll_to_bottom(&mut self) {
        self.scroll_offset = usize::MAX;
    }

    pub(super) const fn is_at_bottom(&self) -> bool {
        self.scroll_offset == usize::MAX
    }

    fn add_context_usage(&mut self, usage: &Usage) {
        if usage.total_tokens > 0 {
            self.context_tokens = Some(
                self.context_tokens
                    .unwrap_or_default()
                    .saturating_add(usage.total_tokens),
            );
        }
    }

    pub(super) fn pasted_image_count(&self) -> usize {
        self.pasted_images.len()
    }

    pub(super) fn pasted_image_labels(&self) -> Vec<String> {
        self.pasted_images
            .iter()
            .map(|path| compact_path(path, 54))
            .collect()
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
            .push("Interrupt requested. Cancelling active run.".to_string());
    }

    pub(super) fn show_run_active_hint(&mut self) {
        self.status = "RUNNING".to_string();
        self.phase = "run active; press Ctrl-C to interrupt".to_string();
    }

    pub(super) fn context_percent_label(&self) -> String {
        match (self.context_tokens, self.context_window) {
            (Some(tokens), Some(window)) if window > 0 => {
                format!(
                    "{}%",
                    tokens.saturating_mul(100).saturating_add(window / 2) / window
                )
            }
            (None, Some(window)) if window > 0 => "0%".to_string(),
            _ => "--%".to_string(),
        }
    }

    pub(crate) fn complete_goal_iteration(&mut self, output: &str) -> GoalIterationOutcome {
        if !self.goal_active {
            return GoalIterationOutcome::Inactive;
        }
        self.goal_iteration = self.goal_iteration.saturating_add(1);
        if output.contains("[GOAL_COMPLETE]") {
            self.goal_active = false;
            self.body.push(format!(
                "[SYS] [Goal] Task completed in {} iteration(s)",
                self.goal_iteration
            ));
            return GoalIterationOutcome::Complete;
        }
        if self.goal_iteration >= self.goal_max_iterations {
            self.goal_active = false;
            self.body.push(format!(
                "[SYS] [Goal] Reached max iterations ({}). Task may be incomplete. You can run /goal again to continue.",
                self.goal_iteration
            ));
            return GoalIterationOutcome::MaxIterations;
        }
        self.body.push(format!(
            "[SYS] [Goal] Iteration {}/{}",
            self.goal_iteration, self.goal_max_iterations
        ));
        let Some(task) = self.goal_task.clone() else {
            self.goal_active = false;
            return GoalIterationOutcome::MaxIterations;
        };
        GoalIterationOutcome::Continue(goal_continuation_prompt(
            &task,
            self.goal_iteration,
            self.goal_max_iterations,
        ))
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum GoalIterationOutcome {
    Inactive,
    Complete,
    MaxIterations,
    Continue(String),
}

fn goal_continuation_prompt(task: &str, iteration: usize, max_iterations: usize) -> String {
    format!(
        "Continue working toward the active goal.\n\n<objective>\n{task}\n</objective>\n\n<goal-check>\nCurrent iteration: {iteration}/{max_iterations}.\nIf the goal is fully complete, include [GOAL_COMPLETE] on its own line.\nOtherwise, make concrete progress and continue.\n</goal-check>"
    )
}

#[derive(Debug, Eq, PartialEq)]
enum LocalCommandOutcome {
    None,
    Consumed,
    Submit(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SubmissionKind {
    Message,
    Queued,
    Steering,
}

fn assistant_content_line(line: impl AsRef<str>) -> String {
    format!("{ASSISTANT_CONTENT_PREFIX}{}", line.as_ref())
}

fn is_assistant_content_line(line: &str) -> bool {
    line.starts_with(ASSISTANT_CONTENT_PREFIX)
}

fn is_thinking_quote_line(line: &str) -> bool {
    line.strip_prefix(ASSISTANT_CONTENT_PREFIX)
        .unwrap_or(line)
        .starts_with('>')
}

fn append_delta_segments(body: &mut Vec<String>, delta: &str, new_line: impl Fn(&str) -> String) {
    for segment in delta.split_inclusive('\n') {
        if segment.ends_with('\n') {
            let trimmed = segment.trim_end_matches('\n').trim_end_matches('\r');
            if !trimmed.is_empty() {
                if let Some(last) = body.last_mut() {
                    last.push_str(trimmed);
                }
            }
            body.push(new_line(""));
        } else if let Some(last) = body.last_mut() {
            last.push_str(segment);
        }
    }
}

fn streaming_part_kind(part_kind: &str) -> StreamingPartKind {
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

fn merge_stream_fragment(current: Option<&str>, fragment: &str) -> String {
    match current {
        Some(current) if !current.is_empty() && fragment.starts_with(current) => {
            fragment.to_string()
        }
        Some(current) => format!("{current}{fragment}"),
        None => fragment.to_string(),
    }
}

fn format_streaming_tool_call_line(state: Option<&StreamingToolCallState>) -> String {
    let name = state
        .and_then(|state| state.name.as_deref())
        .filter(|name| !name.is_empty())
        .unwrap_or("tool");
    let arguments = state.map_or("", |state| state.arguments.trim());
    if arguments.is_empty() || arguments == "{}" || arguments == "null" {
        format!("Tool call: {name}")
    } else {
        format!("Tool call: {name} {arguments}")
    }
}

fn tool_call_visibility_key(call: &starweaver_model::ToolCallPart) -> String {
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

fn model_choice_label(choice: &ModelChoice) -> String {
    if choice.display_name() == choice.model_id {
        choice.model_id.clone()
    } else {
        format!("{} ({})", choice.display_name(), choice.model_id)
    }
}

fn model_choice_config_suffix(choice: &ModelChoice) -> String {
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

fn format_tool_call_line(call: &starweaver_model::ToolCallPart) -> String {
    let arguments = value_preview(&call.arguments.replay_value());
    if arguments == "{}" || arguments == "null" || arguments.is_empty() {
        format!("Tool call: {}", call.name)
    } else {
        format!("Tool call: {} {arguments}", call.name)
    }
}

fn pasted_image_paths(text: &str) -> Vec<String> {
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

fn push_shell_output_lines(body: &mut Vec<String>, label: &str, output: &str) {
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

fn compact_path(path: &str, max_chars: usize) -> String {
    let char_count = path.chars().count();
    if char_count <= max_chars {
        return path.to_string();
    }
    let keep = max_chars.saturating_sub(1);
    let suffix = path
        .chars()
        .rev()
        .take(keep)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!("…{suffix}")
}
