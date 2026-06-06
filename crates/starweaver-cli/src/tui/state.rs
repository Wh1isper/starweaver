use std::{
    collections::{HashMap, HashSet},
    env,
    path::Path,
    time::Instant,
};

use starweaver_core::Usage;
use starweaver_runtime::{AgentStreamEvent, AgentStreamRecord, ModelResponseStreamEvent};

const DEFAULT_CONTEXT_WINDOW_TOKENS: u64 = 200_000;
const MAX_STEERING_ITEMS: usize = 5;

use super::{
    markdown::ASSISTANT_CONTENT_PREFIX,
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
    Help,
}

impl FooterMode {
    pub(super) const fn is_help(&self) -> bool {
        matches!(self, Self::Help)
    }

    pub(super) fn toggle_help(&mut self) {
        *self = match self {
            Self::Context => Self::Help,
            Self::Help => Self::Context,
        };
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
    Other,
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
    visible_tool_calls: HashSet<String>,
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
            visible_tool_calls: HashSet::new(),
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
        self.visible_tool_calls.clear();
        self.footer_mode = FooterMode::Context;
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
        self.visible_tool_calls.clear();
    }

    /// Mark a run failed.
    pub fn fail_run(&mut self, error: &str) {
        self.running = false;
        self.cancel_requested = false;
        self.status = "ERROR".to_string();
        self.phase = "failed".to_string();
        self.streaming_parts.clear();
        self.visible_tool_calls.clear();
        self.body.push(format!("Error: {error}"));
    }

    /// Mark a run cancelled by the interactive user.
    pub fn cancel_run(&mut self, reason: &str) {
        self.running = false;
        self.cancel_requested = false;
        self.status = "IDLE".to_string();
        self.phase = "cancelled".to_string();
        self.streaming_parts.clear();
        self.visible_tool_calls.clear();
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
                            self.push_tool_call(call);
                        }
                        _ => {}
                    }
                }
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
                    StreamingPartKind::Text => "streaming".to_string(),
                    StreamingPartKind::Thinking => "thinking".to_string(),
                    StreamingPartKind::Other => format!("streaming:{}", part.part_kind),
                };
                if matches!(kind, StreamingPartKind::Thinking) {
                    self.body.push("Thinking:".to_string());
                }
            }
            ModelResponseStreamEvent::PartDelta(delta) => {
                match self.streaming_kind_for_delta(delta.index) {
                    StreamingPartKind::Text => {
                        self.phase = "streaming".to_string();
                        self.append_stream_delta(&delta.delta);
                        self.streaming_text_seen = true;
                    }
                    StreamingPartKind::Thinking => {
                        self.phase = "thinking".to_string();
                        self.append_thinking_delta(&delta.delta);
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
            }
        }
    }

    fn append_stream_delta(&mut self, delta: &str) {
        if self.body.is_empty() {
            self.body.push(assistant_content_line(""));
        }
        append_delta_segments(&mut self.body, delta, |line| assistant_content_line(line));
    }

    fn append_thinking_delta(&mut self, delta: &str) {
        if self
            .body
            .last()
            .map_or(true, |line| !line.starts_with("Thinking:"))
        {
            self.body.push("Thinking:".to_string());
        }
        if self.body.last().is_some_and(|line| line == "Thinking:") {
            if let Some(last) = self.body.last_mut() {
                last.push(' ');
            }
        }
        append_delta_segments(&mut self.body, delta, |line| format!("Thinking: {line}"));
    }

    fn streaming_kind_for_delta(&self, index: usize) -> StreamingPartKind {
        self.streaming_parts
            .get(&index)
            .copied()
            .unwrap_or(StreamingPartKind::Other)
    }

    fn push_text_lines(&mut self, text: &str) {
        for line in text.lines() {
            self.body.push(assistant_content_line(line));
        }
    }

    fn push_tool_call(&mut self, call: &starweaver_model::ToolCallPart) {
        self.phase = "tools".to_string();
        if self.visible_tool_calls.insert(call.id.clone()) {
            self.body.push(format_tool_call_line(call));
        }
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
        if self.running && self.composer_has_draft() {
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

    pub(super) fn help_panel_visible(&self) -> bool {
        self.footer_mode.is_help() || self.input.trim_start().starts_with("/help")
    }

    pub(super) fn open_help(&mut self) {
        self.footer_mode = FooterMode::Help;
        self.input_status = Some("help".to_string());
    }

    fn take_local_command(&mut self) -> LocalCommandOutcome {
        let input = self.input.trim().to_string();
        if input == "/help" {
            self.input.clear();
            self.pasted_images.clear();
            self.open_help();
            return LocalCommandOutcome::Consumed;
        }
        if input == "/act" {
            self.input.clear();
            self.run_mode = RunMode::Act;
            self.body.push("[SYS] Mode changed to ACT".to_string());
            self.input_status = Some("mode changed".to_string());
            return LocalCommandOutcome::Consumed;
        }
        if input == "/plan" {
            self.input.clear();
            self.run_mode = RunMode::Plan;
            self.body.push("[SYS] Mode changed to PLAN".to_string());
            self.input_status = Some("mode changed".to_string());
            return LocalCommandOutcome::Consumed;
        }
        if input == "/clear" {
            self.input.clear();
            self.body.clear();
            self.input_status = Some("cleared".to_string());
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
        LocalCommandOutcome::None
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
    } else if normalized.contains("text") || normalized.contains("message") {
        StreamingPartKind::Text
    } else {
        StreamingPartKind::Other
    }
}

fn format_tool_call_line(call: &starweaver_model::ToolCallPart) -> String {
    let arguments = value_preview(&call.arguments);
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
