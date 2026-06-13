use std::{
    collections::{BTreeMap, HashMap, HashSet},
    env,
    fmt::Write as _,
    path::Path,
    process::Command,
    time::Instant,
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use starweaver_context::TASK_SNAPSHOT_EVENT_KIND;
use starweaver_core::{Usage, UsageSnapshot};
use starweaver_model::{PartDelta, StreamDelta};
use starweaver_runtime::{AgentStreamEvent, AgentStreamRecord, ModelResponseStreamEvent};

const DEFAULT_CONTEXT_WINDOW_TOKENS: u64 = 200_000;
const MAX_STEERING_ITEMS: usize = 5;
const SHELL_OUTPUT_MAX_LINES: usize = 200;
const SHELL_STREAM_PREVIEW_MAX_LINES: usize = 6;
const TOOL_PREVIEW_MAX_CHARS: usize = 240;
pub(super) const COMPOSER_VISIBLE_LINES: usize = 5;

use crate::{
    prompt_input::{format_size_bytes, PromptAttachment, PromptInput},
    slash_commands::{expand_slash_command, SlashCommandDefinition},
};

use super::{
    markdown::ASSISTANT_CONTENT_PREFIX,
    render::{snapshot_interactive_lines, truncate_line_center, value_preview},
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum EnterMode {
    Send,
    Newline,
}

impl EnterMode {
    const fn toggle(self) -> Self {
        match self {
            Self::Send => Self::Newline,
            Self::Newline => Self::Send,
        }
    }

    pub(super) const fn sends(self) -> bool {
        matches!(self, Self::Send)
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum PendingSessionCommand {
    Current,
    Select(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum BodyScrollDirection {
    Up,
    Down,
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
pub struct SessionChoice {
    /// Durable session id.
    pub session_id: String,
    /// Optional user-facing title.
    pub title: Option<String>,
    /// Profile recorded on the session.
    pub profile: Option<String>,
    /// Session status.
    pub status: String,
    /// Number of runs in the session.
    pub run_count: usize,
    /// Last output preview, when available.
    pub last_output_preview: Option<String>,
    /// Last update time as persisted by the local store.
    pub updated_at: String,
}

impl SessionChoice {
    pub(super) fn display_title(&self) -> &str {
        self.title.as_deref().unwrap_or("untitled")
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct StreamingToolCallState {
    name: Option<String>,
    arguments: String,
    line_index: Option<usize>,
    linked_call_key: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct HitlPanelState {
    pub(super) tool_call_id: String,
    pub(super) tool_name: String,
    pub(super) command: Option<String>,
    pub(super) risk_level: Option<String>,
    pub(super) reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TaskPanelItem {
    pub id: String,
    pub subject: String,
    pub description: String,
    pub status: String,
    pub active_form: Option<String>,
    pub owner: Option<String>,
    pub blocked_by: Vec<String>,
    pub blocks: Vec<String>,
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
    /// Byte index of the editable prompt cursor.
    pub(super) input_cursor: usize,
    /// Input length last observed after an internal cursor-aware mutation.
    input_cursor_input_len: usize,
    /// Active profile label.
    pub profile: String,
    /// Active model label.
    pub model: String,
    /// Current runtime phase.
    pub phase: String,
    /// True while a background run is active.
    pub running: bool,
    /// Current behavior for the Enter key in the composer.
    pub(super) enter_mode: EnterMode,
    /// Scrollback offset from bottom.
    pub scroll_offset: usize,
    /// Last rendered transcript height, used to keep scroll handling cheap between frames.
    pub(super) rendered_body_len: usize,
    /// Last rendered body viewport height, used to keep scroll handling cheap between frames.
    pub(super) body_viewport_height: usize,
    /// Multiline composer scrollback offset from the bottom of the draft.
    pub(super) input_scroll_offset: usize,
    /// Short-lived composer status for paste, media attach, and steering actions.
    pub(super) input_status: Option<String>,
    /// Image attachments queued into the fixed composer.
    pub(super) pending_attachments: Vec<PromptAttachment>,
    pub(super) run_mode: RunMode,
    pub(super) history: Vec<String>,
    pub(super) history_index: Option<usize>,
    pub(super) history_draft: String,
    streaming_parts: HashMap<usize, StreamingPartKind>,
    streaming_text_seen: bool,
    streaming_reasoning_seen: bool,
    visible_text_seen: bool,
    streaming_tool_calls: HashMap<usize, StreamingToolCallState>,
    visible_tool_calls: HashSet<String>,
    tool_call_arguments: HashMap<String, Value>,
    pending_hitl: Option<HitlPanelState>,
    task_panel_items: Vec<TaskPanelItem>,
    pending_clear_context: bool,
    selection_mode: bool,
    selection_index: Option<usize>,
    pending_submission_display_prompt: Option<String>,
    custom_commands: BTreeMap<String, SlashCommandDefinition>,
    model_choices: Vec<ModelChoice>,
    model_picker_open: bool,
    model_picker_index: usize,
    session_choices: Vec<SessionChoice>,
    session_picker_open: bool,
    session_picker_index: usize,
    pending_session_command: Option<String>,
    pub(super) last_ctrl_c: Option<Instant>,
    pub(super) cancel_requested: bool,
    pub(super) footer_mode: FooterMode,
    pub(super) goal_task: Option<String>,
    pub(super) goal_active: bool,
    pub(super) goal_iteration: usize,
    pub(super) goal_max_iterations: usize,
    pub(super) context_tokens: Option<u64>,
    pub(super) context_window: Option<u64>,
    usage_snapshots: BTreeMap<String, UsageSnapshot>,
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
            input_cursor: 0,
            input_cursor_input_len: 0,
            profile: "general".to_string(),
            model: "local_echo".to_string(),
            phase: "ready".to_string(),
            running: false,
            enter_mode: EnterMode::Send,
            scroll_offset: usize::MAX,
            rendered_body_len: 0,
            body_viewport_height: 1,
            input_scroll_offset: 0,
            input_status: None,
            pending_attachments: Vec::new(),
            run_mode: RunMode::Act,
            history: Vec::new(),
            history_index: None,
            history_draft: String::new(),
            streaming_parts: HashMap::new(),
            streaming_text_seen: false,
            streaming_reasoning_seen: false,
            visible_text_seen: false,
            streaming_tool_calls: HashMap::new(),
            visible_tool_calls: HashSet::new(),
            tool_call_arguments: HashMap::new(),
            pending_hitl: None,
            task_panel_items: Vec::new(),
            pending_clear_context: false,
            selection_mode: false,
            selection_index: None,
            pending_submission_display_prompt: None,
            custom_commands: BTreeMap::new(),
            model_choices: Vec::new(),
            model_picker_open: false,
            model_picker_index: 0,
            session_choices: Vec::new(),
            session_picker_open: false,
            session_picker_index: 0,
            pending_session_command: None,
            last_ctrl_c: None,
            cancel_requested: false,
            footer_mode: FooterMode::Context,
            goal_task: None,
            goal_active: false,
            goal_iteration: 0,
            goal_max_iterations: 10,
            context_tokens: None,
            context_window: Some(DEFAULT_CONTEXT_WINDOW_TOKENS),
            usage_snapshots: BTreeMap::new(),
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

    /// Set session choices shown by `/session`.
    pub fn set_session_choices(&mut self, choices: Vec<SessionChoice>) {
        self.session_choices = choices;
        self.sync_session_picker_index_to_current();
    }

    /// Return recent session choices.
    pub fn session_choices(&self) -> &[SessionChoice] {
        &self.session_choices
    }

    pub(super) const fn session_picker_visible(&self) -> bool {
        self.session_picker_open
    }

    pub(super) const fn session_picker_index(&self) -> usize {
        self.session_picker_index
    }

    pub(crate) fn open_session_picker(&mut self) {
        if self.running {
            self.body.push(
                "[SYS] Session selection is available after the current run finishes.".to_string(),
            );
            self.input_status = Some("session blocked".to_string());
            return;
        }
        self.clear_composer_input();
        self.reset_composer_scroll();
        self.pending_attachments.clear();
        self.footer_mode = FooterMode::Context;
        if self.session_choices.is_empty() {
            self.session_picker_open = false;
            self.body.push("[SYS] No sessions found.".to_string());
            self.input_status = Some("no sessions".to_string());
            return;
        }
        self.model_picker_open = false;
        self.session_picker_open = true;
        self.sync_session_picker_index_to_current();
        self.input_status = Some("session picker".to_string());
    }

    pub(super) fn close_session_picker(&mut self) {
        self.session_picker_open = false;
        self.input_status = Some("session picker closed".to_string());
    }

    pub(super) fn move_session_picker_selection(&mut self, delta: isize) {
        let len = self.session_choices.len();
        if len == 0 {
            self.session_picker_index = 0;
            return;
        }
        let current = self.session_picker_index.min(len.saturating_sub(1));
        let steps = delta.unsigned_abs() % len;
        self.session_picker_index = if delta.is_negative() {
            (current + len - steps) % len
        } else {
            (current + steps) % len
        };
        self.input_status = Some("session picker".to_string());
    }

    pub(super) fn select_session_picker_choice(&mut self) {
        let Some(session_id) = self.selected_session_picker_id() else {
            self.close_session_picker();
            return;
        };
        self.pending_session_command = Some(session_id);
        self.session_picker_open = false;
        self.input_status = Some("session selected".to_string());
    }

    fn selected_session_picker_id(&self) -> Option<String> {
        self.session_choices
            .get(self.session_picker_index)
            .map(|choice| choice.session_id.clone())
    }

    fn sync_session_picker_index_to_current(&mut self) {
        self.session_picker_index = self
            .session_id
            .as_deref()
            .and_then(|session_id| {
                self.session_choices
                    .iter()
                    .position(|choice| choice.session_id == session_id)
            })
            .unwrap_or(0)
            .min(self.session_choices.len().saturating_sub(1));
    }

    pub(super) fn take_pending_session_command(&mut self) -> Option<PendingSessionCommand> {
        self.pending_session_command.take().map(|requested| {
            if requested.is_empty() {
                PendingSessionCommand::Current
            } else {
                PendingSessionCommand::Select(requested)
            }
        })
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
        self.clear_composer_input();
        self.reset_composer_scroll();
        self.pending_attachments.clear();
        self.footer_mode = FooterMode::Context;
        self.session_picker_open = false;
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
        self.pending_attachments.clear();
        self.reset_composer_scroll();
        self.status = snapshot
            .terminal_status
            .clone()
            .unwrap_or_else(|| "IDLE".to_string())
            .to_ascii_uppercase();
        self.model_picker_open = false;
        self.session_picker_open = false;
        self.selection_mode = false;
        self.selection_index = None;
        self.pending_submission_display_prompt = None;
        self.task_panel_items.clone_from(&snapshot.tasks);
        self.sync_session_picker_index_to_current();
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
        self.visible_text_seen = false;
        self.streaming_tool_calls.clear();
        self.visible_tool_calls.clear();
        self.tool_call_arguments.clear();
        self.pending_hitl = None;
        self.task_panel_items.clear();
        self.selection_mode = false;
        self.selection_index = None;
        self.pending_submission_display_prompt = None;
        self.footer_mode = FooterMode::Context;
        self.model_picker_open = false;
        self.session_picker_open = false;
        self.input_status = None;
        self.pending_attachments.clear();
        self.scroll_to_bottom();
        self.body.push(String::new());
        push_user_prompt_lines(&mut self.body, prompt);
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
        self.tool_call_arguments.clear();
        self.pending_hitl = None;
        self.pending_submission_display_prompt = None;
        self.model_picker_open = false;
        self.session_picker_open = false;
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
        self.tool_call_arguments.clear();
        self.pending_hitl = None;
        self.pending_submission_display_prompt = None;
        self.model_picker_open = false;
        self.session_picker_open = false;
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
        self.tool_call_arguments.clear();
        self.pending_hitl = None;
        self.pending_submission_display_prompt = None;
        self.model_picker_open = false;
        self.session_picker_open = false;
        self.body.push(format!("Run cancelled: {reason}"));
    }

    /// Apply a live runtime stream event to the view state.
    pub fn apply_stream_record(&mut self, record: &AgentStreamRecord) {
        let should_auto_scroll = !self.selection_mode;
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
                self.streaming_parts.clear();
                self.streaming_tool_calls.clear();
                self.tool_call_arguments.clear();
                self.streaming_text_seen = false;
                self.streaming_reasoning_seen = false;
            }
            AgentStreamEvent::ModelStream { event, .. } => self.apply_model_stream_event(event),
            AgentStreamEvent::ModelResponse { response, .. } => {
                self.phase = "response".to_string();
                self.update_context_usage(&response.usage);
                self.apply_model_response_parts(&response.parts);
            }
            AgentStreamEvent::ToolCall { call, .. } => {
                self.push_tool_call(call);
            }
            AgentStreamEvent::ToolReturn { tool_return, .. } => {
                self.phase = "tools".to_string();
                let arguments = self.tool_call_arguments.remove(&tool_return.tool_call_id);
                self.update_hitl_panel(tool_return);
                self.update_task_panel_from_tool_return(tool_return);
                self.body
                    .extend(format_tool_return_lines(tool_return, arguments.as_ref()));
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
                if event.kind == "usage_snapshot" {
                    self.apply_usage_snapshot_payload(&event.payload, record.sequence);
                } else if is_task_snapshot_event(&event.kind) {
                    self.apply_task_snapshot_payload(&event.payload);
                } else if let Some(lines) =
                    format_custom_context_event_lines(&event.kind, &event.payload)
                {
                    self.body.extend(lines);
                } else if event.kind == "steering_received" {
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
                    if let Some(text) = text.filter(|text| !text.trim().is_empty()) {
                        self.body.push(format!("Steering received: {text}"));
                    } else {
                        self.body.push("Steering received".to_string());
                    }
                }
            }
            AgentStreamEvent::RunComplete { output, .. } => {
                self.phase = "completed".to_string();
                if !self.visible_text_seen && !output.trim().is_empty() {
                    self.push_text_lines(output);
                    self.visible_text_seen = true;
                }
            }
            AgentStreamEvent::RunFailed { message, .. } => {
                self.status = "FAILED".to_string();
                self.phase = "failed".to_string();
                self.body.push(format!("Run failed: {message}"));
            }
            AgentStreamEvent::NodeComplete { .. } => {}
        }
        if should_auto_scroll {
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
                        self.begin_streaming_tool_call_line(part.index);
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
                        self.visible_text_seen = true;
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
                        self.visible_text_seen = true;
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
                starweaver_model::ModelResponsePart::Text { text }
                | starweaver_model::ModelResponsePart::ProviderText { text, .. }
                    if !self.streaming_text_seen =>
                {
                    self.push_text_lines(text);
                    self.streaming_text_seen = true;
                    self.visible_text_seen = true;
                }
                starweaver_model::ModelResponsePart::Thinking { text, .. }
                | starweaver_model::ModelResponsePart::ProviderThinking { text, .. }
                    if !self.streaming_reasoning_seen =>
                {
                    self.push_thinking_lines(text);
                    self.streaming_reasoning_seen = true;
                }
                starweaver_model::ModelResponsePart::ToolCall(call)
                | starweaver_model::ModelResponsePart::ProviderToolCall { call, .. } => {
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

    fn begin_streaming_tool_call_line(&mut self, index: usize) {
        if self
            .streaming_tool_calls
            .get(&index)
            .is_some_and(|state| state.line_index.is_some() && state.linked_call_key.is_none())
        {
            return;
        }
        let line_index = self.body.len();
        self.streaming_tool_calls.insert(
            index,
            StreamingToolCallState {
                line_index: Some(line_index),
                ..StreamingToolCallState::default()
            },
        );
        self.body.push(format_streaming_tool_call_line(
            self.streaming_tool_calls.get(&index),
        ));
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
        if !call.id.is_empty() {
            self.tool_call_arguments
                .insert(call.id.clone(), call.arguments.replay_value());
        }
        if !self.visible_tool_calls.insert(key.clone()) {
            return;
        }
        let line = format_tool_call_line(call);
        if let Some(line_index) = self.matching_streamed_tool_line(call, &key) {
            if let Some(existing) = self.body.get_mut(line_index) {
                *existing = line;
                return;
            }
        }
        self.body.push(line);
    }

    fn matching_streamed_tool_line(
        &mut self,
        call: &starweaver_model::ToolCallPart,
        key: &str,
    ) -> Option<usize> {
        let linked_index = self
            .streaming_tool_calls
            .iter()
            .filter(|(_, state)| state.linked_call_key.as_deref() == Some(key))
            .map(|(index, _)| *index)
            .min();
        let matching_arguments_index = linked_index.or_else(|| {
            self.streaming_tool_calls
                .iter()
                .filter(|(_, state)| streaming_tool_state_is_available(state, key))
                .filter(|(_, state)| state.name.as_deref() == Some(call.name.as_str()))
                .filter(|(_, state)| streaming_tool_arguments_match(state.arguments.trim(), call))
                .map(|(index, _)| *index)
                .min()
        });
        let fallback_index = matching_arguments_index.or_else(|| {
            self.streaming_tool_calls
                .iter()
                .filter(|(_, state)| streaming_tool_state_is_available(state, key))
                .filter(|(_, state)| state.name.as_deref() == Some(call.name.as_str()))
                .map(|(index, _)| *index)
                .min()
        })?;
        let state = self.streaming_tool_calls.get_mut(&fallback_index)?;
        state.linked_call_key = Some(key.to_string());
        state.line_index
    }

    fn update_hitl_panel(&mut self, tool_return: &starweaver_model::ToolReturnPart) {
        if tool_return
            .metadata
            .get("control_flow")
            .and_then(Value::as_str)
            != Some("approval_required")
        {
            return;
        }
        let approval = tool_return.metadata.get("approval");
        self.status = "WAITING".to_string();
        self.phase = "hitl approval".to_string();
        self.pending_hitl = Some(HitlPanelState {
            tool_call_id: tool_return.tool_call_id.clone(),
            tool_name: tool_return.name.clone(),
            command: approval
                .and_then(|value| value.get("command"))
                .and_then(Value::as_str)
                .map(str::to_string),
            risk_level: approval
                .and_then(|value| value.get("risk_level"))
                .and_then(Value::as_str)
                .map(str::to_string),
            reason: approval
                .and_then(|value| value.get("reason"))
                .and_then(Value::as_str)
                .map(str::to_string),
        });
    }

    fn update_task_panel_from_tool_return(
        &mut self,
        tool_return: &starweaver_model::ToolReturnPart,
    ) {
        if !is_task_tool_name(&tool_return.name) {
            return;
        }
        if let Some(items) = task_panel_items_from_value(&tool_return.content) {
            self.task_panel_items = items;
        }
    }

    fn apply_task_snapshot_payload(&mut self, payload: &Value) {
        if let Some(items) = task_panel_items_from_value(payload) {
            self.task_panel_items = items;
        }
    }

    pub(super) fn apply_paste(&mut self, text: &str) {
        self.footer_mode = FooterMode::Context;
        let image_paths = pasted_image_paths(text);
        if image_paths.is_empty() {
            self.insert_composer_str(text);
            self.input_status = Some(format!("pasted {} chars", text.chars().count()));
            return;
        }

        self.move_composer_cursor_to_end();
        if !self.input.is_empty() && !self.input.ends_with([' ', '\n']) {
            self.insert_composer_str(" ");
        }
        for path in image_paths {
            if !self.input.is_empty() && !self.input.ends_with([' ', '\n']) {
                self.insert_composer_str(" ");
            }
            self.insert_composer_str(&path);
        }
        self.input_status = Some("image path pasted".to_string());
    }

    pub(crate) fn attach_image(&mut self, attachment: PromptAttachment) {
        let placeholder = attachment.placeholder.clone();
        self.move_composer_cursor_to_end();
        if !self.input.is_empty() && !self.input.ends_with([' ', '\n']) {
            self.insert_composer_str(" ");
        }
        self.insert_composer_str(&placeholder);
        self.insert_composer_str(" ");
        self.pending_attachments.push(attachment);
        self.reset_composer_scroll();
        let count = self.pending_attachments.len();
        self.input_status = Some(if count == 1 {
            format!(
                "image attached: {}",
                self.pending_attachments[0].description()
            )
        } else {
            let total_size = self
                .pending_attachments
                .iter()
                .map(|attachment| attachment.size_bytes)
                .sum::<usize>();
            format!(
                "images attached: {count} ({})",
                format_size_bytes(total_size)
            )
        });
    }

    pub(super) fn take_submission_prompt(&mut self) -> Option<PromptInput> {
        self.take_prompt(SubmissionKind::Message)
    }

    pub(super) fn take_steering_prompt(&mut self) -> Option<SteeringSubmission> {
        self.retain_visible_attachments();
        if !self.pending_attachments.is_empty() {
            self.input_status = Some("image steering unsupported while running".to_string());
            return None;
        }
        self.take_prompt(SubmissionKind::Steering)
            .map(|input| self.record_steering_message(input.display_text()))
    }

    pub(crate) fn take_pending_submission_display_prompt(&mut self) -> Option<String> {
        self.pending_submission_display_prompt.take()
    }

    pub(super) fn take_pending_clear_context(&mut self) -> bool {
        std::mem::take(&mut self.pending_clear_context)
    }

    pub(crate) fn clear_context_view(&mut self) {
        self.session_id = None;
        self.body.clear();
        self.context_tokens = None;
        self.streaming_parts.clear();
        self.streaming_text_seen = false;
        self.streaming_reasoning_seen = false;
        self.visible_text_seen = false;
        self.streaming_tool_calls.clear();
        self.visible_tool_calls.clear();
        self.tool_call_arguments.clear();
        self.pending_hitl = None;
        self.task_panel_items.clear();
        self.steering_items.clear();
        self.goal_task = None;
        self.goal_active = false;
        self.phase = "cleared".to_string();
        self.status = "IDLE".to_string();
        self.scroll_to_bottom();
    }

    pub(super) fn take_paste_image_command(&mut self) -> bool {
        if self.input.trim() != "/paste-image" {
            return false;
        }
        self.input.clear();
        self.reset_composer_scroll();
        self.footer_mode = FooterMode::Context;
        true
    }

    fn take_prompt(&mut self, kind: SubmissionKind) -> Option<PromptInput> {
        self.retain_visible_attachments();
        let command = self.take_local_command();
        if matches!(
            command,
            LocalCommandOutcome::Consumed | LocalCommandOutcome::PasteImage
        ) {
            return None;
        }
        let prompt = match command {
            LocalCommandOutcome::Submit(prompt) => prompt,
            LocalCommandOutcome::Consumed | LocalCommandOutcome::PasteImage => {
                unreachable!("handled above")
            }
            LocalCommandOutcome::None => self.input.trim().to_string(),
        };
        if prompt.is_empty() && self.pending_attachments.is_empty() {
            self.clear_composer_input();
            self.reset_composer_scroll();
            return None;
        }
        let attachments = std::mem::take(&mut self.pending_attachments);
        self.clear_composer_input();
        self.reset_composer_scroll();
        match kind {
            SubmissionKind::Message => self.input_status = Some("message sent".to_string()),
            SubmissionKind::Steering => {
                self.input_status = Some("steer sent".to_string());
            }
        }
        Some(PromptInput {
            text: prompt,
            attachments,
            extra_text_parts: Vec::new(),
        })
    }

    pub(super) fn clear_composer(&mut self) {
        self.clear_composer_input();
        self.reset_composer_scroll();
        self.pending_attachments.clear();
        self.input_status = None;
        self.history_index = None;
        self.footer_mode = FooterMode::Context;
    }

    pub(super) fn backspace_composer(&mut self) {
        if self.remove_trailing_attachment_placeholder() {
            return;
        }
        let cursor = self.composer_cursor_byte();
        let Some(previous) = self.input[..cursor]
            .char_indices()
            .last()
            .map(|(index, _)| index)
        else {
            self.remove_last_pasted_image();
            return;
        };
        self.input.replace_range(previous..cursor, "");
        self.input_cursor = previous;
        self.input_cursor_input_len = self.input.len();
        self.reset_composer_scroll();
    }

    fn retain_visible_attachments(&mut self) {
        self.pending_attachments
            .retain(|attachment| self.input.contains(&attachment.placeholder));
    }

    fn remove_trailing_attachment_placeholder(&mut self) -> bool {
        let Some(attachment) = self.pending_attachments.last() else {
            return false;
        };
        let trimmed_input = self.input.trim_end_matches([' ', '\n']);
        let Some(prefix) = trimmed_input.strip_suffix(&attachment.placeholder) else {
            return false;
        };
        self.input.truncate(prefix.len());
        self.input_cursor = self.input.len();
        self.input_cursor_input_len = self.input.len();
        self.reset_composer_scroll();
        self.remove_last_pasted_image();
        true
    }

    fn remove_last_pasted_image(&mut self) {
        if self.pending_attachments.pop().is_some() {
            self.input_status = Some(if self.pending_attachments.is_empty() {
                "image detached".to_string()
            } else {
                format!("images attached: {}", self.pending_attachments.len())
            });
        }
    }

    pub(super) fn composer_is_empty(&self) -> bool {
        self.input.is_empty() && self.pending_attachments.is_empty()
    }

    pub(super) fn composer_has_draft(&self) -> bool {
        !self.input.trim().is_empty() || !self.pending_attachments.is_empty()
    }

    pub(super) fn toggle_enter_mode(&mut self) {
        self.enter_mode = self.enter_mode.toggle();
        self.input_status = Some(match self.enter_mode {
            EnterMode::Send => "Enter sends".to_string(),
            EnterMode::Newline => "Enter inserts newline".to_string(),
        });
    }

    pub(super) const fn enter_sends(&self) -> bool {
        self.enter_mode.sends()
    }

    pub(super) const fn enter_action_label(&self) -> &'static str {
        match (self.running, self.enter_mode) {
            (true, EnterMode::Send) => "Enter: Steer",
            (false, EnterMode::Send) => "Enter: Send",
            (_, EnterMode::Newline) => "Enter: Newline",
        }
    }

    pub(super) const fn enter_toggle_label(&self) -> &'static str {
        match (self.running, self.enter_mode) {
            (_, EnterMode::Send) => "Tab: Enter inserts newline",
            (true, EnterMode::Newline) => "Tab: Enter steers",
            (false, EnterMode::Newline) => "Tab: Enter sends",
        }
    }

    pub(super) fn input_mode_label(&self) -> &'static str {
        if self.selection_mode {
            "SELECT"
        } else if self.session_picker_open {
            "SESSION"
        } else if self.model_picker_open {
            "MODEL"
        } else if self.running && self.composer_has_draft() {
            "STEER"
        } else if self.running {
            "RUNNING"
        } else {
            self.run_mode.label()
        }
    }

    pub(super) const fn selection_mode_visible(&self) -> bool {
        self.selection_mode
    }

    pub(super) fn open_selection_mode(&mut self) {
        self.model_picker_open = false;
        self.session_picker_open = false;
        self.footer_mode = FooterMode::Context;
        self.selection_mode = true;
        if self
            .selection_index
            .map_or(true, |index| index >= self.body.len())
        {
            self.selection_index = self.last_selectable_body_index();
        }
        self.input_status = Some("selection mode".to_string());
    }

    pub(super) fn close_selection_mode(&mut self) {
        self.selection_mode = false;
        self.input_status = Some("selection closed".to_string());
    }

    pub(super) fn move_selection(&mut self, delta: isize) {
        let selectable = self.selectable_body_indices();
        if selectable.is_empty() {
            self.selection_index = None;
            self.input_status = Some("selection empty".to_string());
            return;
        }
        let current_index = self
            .selection_index
            .and_then(|index| selectable.iter().position(|candidate| *candidate == index))
            .unwrap_or_else(|| selectable.len().saturating_sub(1));
        let len = selectable.len();
        let steps = delta.unsigned_abs() % len;
        let next_index = if delta.is_negative() {
            (current_index + len - steps) % len
        } else {
            (current_index + steps) % len
        };
        self.selection_index = selectable.get(next_index).copied();
        self.input_status = Some("selection mode".to_string());
    }

    pub(super) fn selected_line_preview(&self) -> Option<String> {
        self.selection_index
            .and_then(|index| self.body.get(index))
            .map(|line| compact_status_text(body_line_display_text(line), 96))
    }

    pub(super) fn selection_position_label(&self) -> Option<String> {
        let selectable = self.selectable_body_indices();
        let selected = self.selection_index?;
        let position = selectable
            .iter()
            .position(|candidate| *candidate == selected)?
            .saturating_add(1);
        Some(format!("{position}/{}", selectable.len()))
    }

    fn selectable_body_indices(&self) -> Vec<usize> {
        self.body
            .iter()
            .enumerate()
            .filter_map(|(index, line)| (!line.trim().is_empty()).then_some(index))
            .collect()
    }

    fn last_selectable_body_index(&self) -> Option<usize> {
        self.body.iter().rposition(|line| !line.trim().is_empty())
    }

    #[cfg(test)]
    pub(super) fn input_status_text(&self) -> &str {
        self.input_status.as_deref().unwrap_or(&self.phase)
    }

    pub(super) const fn help_panel_visible() -> bool {
        false
    }

    /// Set config-defined slash commands shown by `/help` and expanded before submit.
    pub fn set_custom_commands(&mut self, commands: BTreeMap<String, SlashCommandDefinition>) {
        self.custom_commands = commands;
        self.footer_mode = FooterMode::Context;
    }

    #[allow(clippy::too_many_lines)]
    fn take_local_command(&mut self) -> LocalCommandOutcome {
        let input = self.input.trim().to_string();
        if input == "/help" {
            self.clear_composer_input();
            self.pending_attachments.clear();
            self.footer_mode = FooterMode::Context;
            self.append_help_to_body();
            self.input_status = Some("help".to_string());
            return LocalCommandOutcome::Consumed;
        }
        if input == "/clear" {
            self.clear_composer_input();
            self.clear_context_view();
            self.pending_clear_context = true;
            self.footer_mode = FooterMode::Context;
            self.input_status = Some("context cleared".to_string());
            return LocalCommandOutcome::Consumed;
        }
        if input == "/cost" {
            self.clear_composer_input();
            self.pending_attachments.clear();
            self.footer_mode = FooterMode::Context;
            self.append_cost_summary();
            self.input_status = Some("cost".to_string());
            return LocalCommandOutcome::Consumed;
        }
        if input == "/model" || input.starts_with("/model ") {
            self.clear_composer_input();
            self.pending_attachments.clear();
            self.footer_mode = FooterMode::Context;
            self.handle_model_command(input.strip_prefix("/model").unwrap_or_default().trim());
            if !self.model_picker_open {
                self.input_status = Some("model".to_string());
            }
            return LocalCommandOutcome::Consumed;
        }
        if input == "/session" || input.starts_with("/session ") {
            self.clear_composer_input();
            self.pending_attachments.clear();
            self.footer_mode = FooterMode::Context;
            self.handle_session_command(input.strip_prefix("/session").unwrap_or_default().trim());
            return LocalCommandOutcome::Consumed;
        }
        if input == "/paste-image" {
            self.clear_composer_input();
            self.footer_mode = FooterMode::Context;
            return LocalCommandOutcome::PasteImage;
        }
        if let Some(command) = input.strip_prefix('!') {
            self.clear_composer_input();
            self.pending_attachments.clear();
            self.footer_mode = FooterMode::Context;
            self.run_shell_command(command.trim());
            self.input_status = Some("shell".to_string());
            return LocalCommandOutcome::Consumed;
        }
        if input == "/goal" || input.starts_with("/goal ") {
            self.clear_composer_input();
            let task = input.strip_prefix("/goal").unwrap_or_default().trim();
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
        if let Some(expanded) = expand_slash_command(&self.custom_commands, &input) {
            self.clear_composer_input();
            self.footer_mode = FooterMode::Context;
            let mut message = format!("[SYS] Expanded /{} custom command", expanded.command_name);
            if expanded.invoked_name != expanded.command_name {
                let _ = write!(message, " (alias /{})", expanded.invoked_name);
            }
            if let Some(description) = expanded
                .description
                .as_deref()
                .filter(|value| !value.trim().is_empty())
            {
                message.push_str(": ");
                message.push_str(description.trim());
            }
            self.body.push(message);
            self.input_status = Some(format!("command /{}", expanded.command_name));
            self.pending_submission_display_prompt = Some(expanded.prompt);
            return LocalCommandOutcome::Submit(input);
        }
        if input.starts_with('/') {
            self.clear_composer_input();
            self.pending_attachments.clear();
            self.footer_mode = FooterMode::Context;
            self.body.push(format!(
                "[SYS] Unknown command: {input}. Available commands: {}",
                self.available_command_summary()
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
            "  /clear            Clear transcript and start a fresh context".to_string(),
            "  /cost             Show usage and context".to_string(),
            "  /model [profile]  Open or select a model profile".to_string(),
            "  /session [id]     Open session selector or reload a session".to_string(),
            "  /goal <task>      Run toward a verified goal".to_string(),
            "  /paste-image      Attach image from system clipboard".to_string(),
            "  !<command>        Run a shell command inline".to_string(),
        ]);
        let custom_commands = self.custom_command_definitions();
        if !custom_commands.is_empty() {
            self.body.push(String::new());
            self.body.push("Custom commands".to_string());
            for command in custom_commands {
                let description = command
                    .description
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or("Run configured prompt");
                let aliases = if command.aliases.is_empty() {
                    String::new()
                } else {
                    format!(
                        " (aliases: {})",
                        command
                            .aliases
                            .iter()
                            .map(|alias| format!("/{alias}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                };
                self.body.push(format!(
                    "  /{:<16} {}{}",
                    format!("{} [instruction]", command.name),
                    description,
                    aliases
                ));
            }
        }
        self.body.extend([
            String::new(),
            "Shortcuts".to_string(),
            "  Up/Down           Browse prompt history".to_string(),
            "  PageUp/PageDown   Scroll transcript".to_string(),
            "  Mouse wheel       Scroll transcript".to_string(),
            "  Enter             Send message or select model/session".to_string(),
            "  Tab               Queue a draft while running".to_string(),
            "  Ctrl+C            Interrupt or exit".to_string(),
        ]);
    }

    fn available_command_summary(&self) -> String {
        let mut commands = vec![
            "/help".to_string(),
            "/clear".to_string(),
            "/cost".to_string(),
            "/model".to_string(),
            "/session".to_string(),
            "/goal".to_string(),
            "/paste-image".to_string(),
            "!<command>".to_string(),
        ];
        commands.extend(
            self.custom_command_definitions()
                .into_iter()
                .map(|command| format!("/{}", command.name)),
        );
        commands.join(", ")
    }

    fn custom_command_definitions(&self) -> Vec<SlashCommandDefinition> {
        let mut definitions = self
            .custom_commands
            .values()
            .cloned()
            .collect::<Vec<SlashCommandDefinition>>();
        definitions.sort_by(|left, right| left.name.cmp(&right.name));
        definitions.dedup_by(|left, right| left.name == right.name);
        definitions
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

    fn handle_session_command(&mut self, requested: &str) {
        if self.running {
            self.body.push(
                "[SYS] Session selection is available after the current run finishes.".to_string(),
            );
            self.input_status = Some("session blocked".to_string());
            return;
        }
        self.model_picker_open = false;
        self.session_picker_open = false;
        self.pending_session_command = Some(requested.to_string());
        self.input_status = Some(if requested.is_empty() {
            "session".to_string()
        } else {
            "session reload".to_string()
        });
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
        self.body.extend(self.format_cost_summary_lines());
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

    pub(super) const fn pending_hitl(&self) -> Option<&HitlPanelState> {
        self.pending_hitl.as_ref()
    }

    pub(super) fn task_panel_items(&self) -> &[TaskPanelItem] {
        &self.task_panel_items
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
        self.body.push(format!("Steering: {text}"));
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

    pub(super) const fn update_render_metrics(
        &mut self,
        rendered_body_len: usize,
        body_viewport_height: usize,
    ) {
        self.rendered_body_len = rendered_body_len;
        self.body_viewport_height = body_viewport_height;
    }

    pub(super) fn scroll_body(&mut self, amount: usize, direction: BodyScrollDirection) -> bool {
        let previous = self.scroll_offset;
        let body_height = self.body_viewport_height.max(1);
        let max_scroll = self.rendered_body_len.saturating_sub(body_height);
        let current = if self.is_at_bottom() {
            max_scroll
        } else {
            self.scroll_offset.min(max_scroll)
        };
        let next = match direction {
            BodyScrollDirection::Up => current.saturating_sub(amount),
            BodyScrollDirection::Down => current.saturating_add(amount),
        };
        if next >= max_scroll {
            self.scroll_to_bottom();
        } else {
            self.scroll_offset = next;
        }
        self.scroll_offset != previous
    }

    pub(super) const fn composer_scroll_offset(&self) -> usize {
        self.input_scroll_offset
    }

    pub(super) fn reset_composer_scroll(&mut self) {
        self.input_scroll_offset = 0;
    }

    pub(super) fn scroll_composer_up(&mut self, amount: usize) {
        self.input_scroll_offset = self.input_scroll_offset.saturating_add(amount);
    }

    pub(super) fn scroll_composer_down(&mut self, amount: usize) {
        self.input_scroll_offset = self.input_scroll_offset.saturating_sub(amount);
    }

    fn clear_composer_input(&mut self) {
        self.input.clear();
        self.input_cursor = 0;
        self.input_cursor_input_len = 0;
    }

    pub(super) fn composer_cursor_byte(&self) -> usize {
        if self.input_cursor_input_len != self.input.len() {
            return self.input.len();
        }
        previous_char_boundary(&self.input, self.input_cursor.min(self.input.len()))
    }

    pub(super) fn move_composer_cursor_left(&mut self) {
        let cursor = self.composer_cursor_byte();
        if let Some(previous) = self.input[..cursor]
            .char_indices()
            .last()
            .map(|(index, _)| index)
        {
            self.input_cursor = previous;
            self.input_cursor_input_len = self.input.len();
            self.reset_composer_scroll();
        }
    }

    pub(super) fn move_composer_cursor_right(&mut self) {
        let cursor = self.composer_cursor_byte();
        if cursor >= self.input.len() {
            self.move_composer_cursor_to_end();
            return;
        }
        let next = self.input[cursor..]
            .chars()
            .next()
            .map_or(self.input.len(), |ch| cursor + ch.len_utf8());
        self.input_cursor = next;
        self.input_cursor_input_len = self.input.len();
        self.reset_composer_scroll();
    }

    pub(super) fn move_composer_cursor_to_line_start(&mut self) {
        let cursor = self.composer_cursor_byte();
        self.input_cursor = self.input[..cursor]
            .rfind('\n')
            .map_or(0, |index| index + 1);
        self.input_cursor_input_len = self.input.len();
        self.reset_composer_scroll();
    }

    pub(super) fn move_composer_cursor_to_line_end(&mut self) {
        let cursor = self.composer_cursor_byte();
        self.input_cursor = self.input[cursor..]
            .find('\n')
            .map_or(self.input.len(), |offset| cursor + offset);
        self.input_cursor_input_len = self.input.len();
        self.reset_composer_scroll();
    }

    pub(super) fn move_composer_cursor_to_end(&mut self) {
        self.input_cursor = self.input.len();
        self.input_cursor_input_len = self.input.len();
    }

    pub(super) fn insert_composer_str(&mut self, text: &str) {
        let cursor = self.composer_cursor_byte();
        self.input.insert_str(cursor, text);
        self.input_cursor = cursor + text.len();
        self.input_cursor_input_len = self.input.len();
        self.reset_composer_scroll();
        self.history_index = None;
    }

    pub(super) fn push_composer_char(&mut self, ch: char) {
        let mut buffer = [0; 4];
        self.insert_composer_str(ch.encode_utf8(&mut buffer));
        self.input_status = None;
    }

    pub(super) fn insert_composer_newline(&mut self) {
        self.insert_composer_str("\n");
    }

    fn update_context_usage(&mut self, usage: &Usage) {
        if usage.total_tokens > 0 {
            self.context_tokens = Some(usage.total_tokens);
        }
    }

    fn apply_usage_snapshot_payload(&mut self, payload: &Value, sequence: usize) {
        if let Ok(snapshot) = serde_json::from_value::<UsageSnapshot>(payload.clone()) {
            let key = if snapshot.run_id.is_empty() {
                format!("sequence:{sequence}")
            } else {
                snapshot.run_id.clone()
            };
            self.usage_snapshots.insert(key, snapshot);
        }
    }

    fn format_cost_summary_lines(&self) -> Vec<String> {
        let mut lines = vec!["[SYS] Token Usage Summary:".to_string(), String::new()];
        lines.push(format!(
            "[SYS] Latest request context tokens: {}",
            format_u64_with_commas(self.context_tokens.unwrap_or_default())
        ));
        if let Some(window) = self.context_window {
            lines.push(format!(
                "[SYS] Context window: {}",
                format_u64_with_commas(window)
            ));
            lines.push(format!(
                "[SYS] Context used: {}",
                self.context_percent_label()
            ));
        }

        let mut model_usages = BTreeMap::<String, Usage>::new();
        let mut agent_usages = BTreeMap::<String, Usage>::new();
        for snapshot in self.usage_snapshots.values() {
            for (model_id, usage) in &snapshot.model_usages {
                model_usages
                    .entry(model_id.clone())
                    .or_default()
                    .add_assign(usage);
            }
            for (agent_id, total) in &snapshot.agent_usages {
                agent_usages
                    .entry(agent_id.clone())
                    .or_default()
                    .add_assign(&total.usage);
            }
        }

        if model_usages.is_empty() && agent_usages.is_empty() {
            lines.push("[SYS] No usage data available.".to_string());
            return lines;
        }

        lines.push(String::new());
        lines.push("[SYS] By Model:".to_string());
        for (model_id, usage) in &model_usages {
            push_usage_entry_lines(&mut lines, model_id, usage);
            lines.push(String::new());
        }

        lines.push("[SYS] By Agent:".to_string());
        for (agent_id, usage) in &agent_usages {
            push_usage_entry_lines(&mut lines, agent_id, usage);
            lines.push(String::new());
        }

        let mut total = Usage::default();
        for usage in model_usages.values() {
            total.add_assign(usage);
        }
        lines.push("[SYS] Total:".to_string());
        lines.push(format!(
            "[SYS]   Input:  {} tokens",
            format_u64_with_commas(total.input_tokens)
        ));
        lines.push(format!(
            "[SYS]   Output: {} tokens",
            format_u64_with_commas(total.output_tokens)
        ));
        if total.cache_write_tokens > 0 {
            lines.push(format!(
                "[SYS]   Cache Write: {} tokens",
                format_u64_with_commas(total.cache_write_tokens)
            ));
        }
        if total.cache_read_tokens > 0 {
            lines.push(format!(
                "[SYS]   Cache Read:  {} tokens",
                format_u64_with_commas(total.cache_read_tokens)
            ));
        }
        lines.push(format!(
            "[SYS]   Total:  {} tokens",
            format_u64_with_commas(total.total_tokens)
        ));
        lines.push(format!("[SYS]   Requests: {}", total.requests));
        if total.tool_calls > 0 {
            lines.push(format!("[SYS]   Tool calls: {}", total.tool_calls));
        }
        lines
    }

    pub(crate) fn pasted_image_count(&self) -> usize {
        self.pending_attachments.len()
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
            self.move_composer_cursor_to_end();
            self.reset_composer_scroll();
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
        self.move_composer_cursor_to_end();
        self.reset_composer_scroll();
    }

    pub(super) fn request_cancel(&mut self) {
        let already_requested = self.cancel_requested;
        self.cancel_requested = true;
        self.status = "INTERRUPT".to_string();
        self.phase = "cancel requested".to_string();
        if !already_requested {
            self.body
                .push("Interrupt requested. Cancelling active run.".to_string());
        }
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
    PasteImage,
    Submit(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SubmissionKind {
    Message,
    Steering,
}

fn push_user_prompt_lines(body: &mut Vec<String>, prompt: &str) {
    let mut lines = prompt.lines();
    if let Some(first) = lines.next() {
        body.push(format!("User: {first}"));
        body.extend(lines.map(|line| format!("  {line}")));
    } else {
        body.push("User:".to_string());
    }
}

fn push_usage_entry_lines(lines: &mut Vec<String>, name: &str, usage: &Usage) {
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
    lines.push(format!("[SYS]     Requests: {}", usage.requests));
    if usage.tool_calls > 0 {
        lines.push(format!("[SYS]     Tool calls: {}", usage.tool_calls));
    }
}

fn format_u64_with_commas(value: u64) -> String {
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
        format!("Tool call: {name} {}", truncate_line_center(arguments, 80))
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

pub(super) fn display_lines_for_stream_record(record: &AgentStreamRecord) -> Vec<String> {
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
        AgentStreamEvent::RunFailed { message, .. } => vec![format!("Run failed: {message}")],
        _ => Vec::new(),
    }
}

fn format_tool_call_line(call: &starweaver_model::ToolCallPart) -> String {
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

fn format_tool_return_lines(
    tool_return: &starweaver_model::ToolReturnPart,
    arguments: Option<&Value>,
) -> Vec<String> {
    let display_value = tool_return
        .user_content
        .as_ref()
        .unwrap_or(&tool_return.content);
    let mut lines = if tool_return.is_error {
        vec![format!(
            "Tool error: {} {}",
            tool_return.name,
            full_value_text(display_value)
        )]
    } else {
        match tool_return.name.as_str() {
            "edit" | "multi_edit" => {
                format_edit_tool_lines(&tool_return.name, arguments, display_value)
            }
            "write" => format_write_tool_lines(display_value, arguments),
            "view" => format_view_tool_lines(display_value, arguments),
            "summarize" => format_summarize_tool_lines(display_value, arguments),
            "shell_exec" | "shell_wait" | "shell_status" | "shell_input" | "shell_signal"
            | "shell_kill" => format_shell_tool_lines(&tool_return.name, display_value, arguments),
            "task_create" | "task_get" | "task_update" | "task_list" => {
                format_task_tool_lines(&tool_return.name, &tool_return.content, display_value)
            }
            _ => format_generic_tool_lines(&tool_return.name, display_value),
        }
    };
    if !is_task_tool_name(&tool_return.name) {
        if let Some(duration) = tool_duration_label(&tool_return.metadata) {
            lines.push(format!("  Duration: {duration}"));
        }
    }
    lines
}

fn format_shell_tool_lines(name: &str, result: &Value, arguments: Option<&Value>) -> Vec<String> {
    let mut lines = vec![format!("Tool result: {name}")];
    if let Some(command) = shell_command(result, arguments) {
        lines.push("  Command:".to_string());
        for line in full_command_lines(command) {
            lines.push(format!("    │ {line}"));
        }
    }
    if let Some(cwd) = result
        .get("cwd")
        .or_else(|| arguments.and_then(|args| args.get("cwd")))
        .and_then(Value::as_str)
        .filter(|cwd| !cwd.trim().is_empty())
    {
        lines.push(format!("  Cwd: {cwd}"));
    }
    if let Some(process_id) = result.get("process_id").and_then(Value::as_str) {
        lines.push(format!("  Process: {process_id}"));
    }
    if let Some(status) = shell_status_text(result) {
        lines.push(format!("  Status: {status}"));
    }
    if let Some(stdout) = result.get("stdout").and_then(Value::as_str) {
        push_shell_stream_preview(&mut lines, "stdout", stdout);
    }
    if let Some(stderr) = result.get("stderr").and_then(Value::as_str) {
        push_shell_stream_preview(&mut lines, "stderr", stderr);
    }
    for field in ["stdout_file_path", "stderr_file_path"] {
        if let Some(path) = result.get(field).and_then(Value::as_str) {
            lines.push(format!("  {field}: {path}"));
        }
    }
    if lines.len() == 1 && !is_empty_result(result) {
        push_indented_preview(&mut lines, &value_text(result), 12);
    }
    lines
}

fn shell_command<'a>(result: &'a Value, arguments: Option<&'a Value>) -> Option<&'a str> {
    result
        .get("command")
        .or_else(|| arguments.and_then(|args| args.get("command")))
        .and_then(Value::as_str)
        .filter(|command| !command.trim().is_empty())
}

fn shell_status_text(result: &Value) -> Option<String> {
    result
        .get("return_code")
        .and_then(Value::as_i64)
        .map(|code| format!("exit {code}"))
        .or_else(|| {
            result
                .get("status")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
}

fn push_shell_stream_preview(lines: &mut Vec<String>, label: &str, output: &str) {
    if output.trim().is_empty() {
        return;
    }
    lines.push(format!("  {label}:"));
    for line in preview_lines(output, SHELL_STREAM_PREVIEW_MAX_LINES) {
        lines.push(format!("    │ {line}"));
    }
}

fn full_command_lines(command: &str) -> Vec<String> {
    let mut lines = command.lines().collect::<Vec<_>>();
    if command.ends_with('\n') || lines.is_empty() {
        lines.push("");
    }
    lines
        .into_iter()
        .flat_map(|line| split_sanitized_line(line, TOOL_PREVIEW_MAX_CHARS))
        .collect()
}

fn split_sanitized_line(line: &str, max_chars: usize) -> Vec<String> {
    let line = sanitize_control_chars(line);
    let max_chars = max_chars.max(1);
    if line.is_empty() {
        return vec![String::new()];
    }
    let mut chunks = Vec::new();
    let mut current = String::new();
    for ch in line.chars() {
        if current.chars().count() >= max_chars {
            chunks.push(std::mem::take(&mut current));
        }
        current.push(ch);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

fn format_generic_tool_lines(name: &str, result: &Value) -> Vec<String> {
    let mut lines = vec![format!("Tool result: {name}")];
    if is_empty_result(result) {
        return lines;
    }
    push_indented_preview(&mut lines, &value_text(result), 12);
    lines
}

fn format_edit_tool_lines(name: &str, arguments: Option<&Value>, result: &Value) -> Vec<String> {
    let mut lines = vec![format!("Tool result: {name}")];
    let Some(args) = arguments else {
        if let Some(file_path) = result_path(result) {
            lines.push(format!("  Editing file: {file_path}"));
        }
        if let Some(status) = edit_result_status(result) {
            lines.push(format!("  Status: {status}"));
        }
        if !is_empty_result(result) {
            lines.push(format!("  Result: {}", value_preview(result)));
        }
        return lines;
    };
    let file_path = file_path_arg(args).unwrap_or("unknown");
    lines.push(format!("  Editing file: {file_path}"));
    let edits = edit_operations(args);
    if edits.is_empty() {
        let old_string = string_field(args, "old_string");
        let new_string = string_field(args, "new_string");
        let is_new_file = old_string.is_empty();
        lines.extend(format_one_edit(
            1,
            old_string,
            new_string,
            false,
            is_new_file,
        ));
    } else {
        let new_files = edits
            .iter()
            .enumerate()
            .filter(|(index, edit)| *index == 0 && edit.old_string.is_empty())
            .count();
        let modifications = edits.len().saturating_sub(new_files);
        let replace_all = edits.iter().filter(|edit| edit.replace_all).count();
        lines.push(format!(
            "  Summary: {} edit{} ({} new file{}, {} modification{}, {} replace-all operation{})",
            edits.len(),
            plural_suffix(edits.len()),
            new_files,
            plural_suffix(new_files),
            modifications,
            plural_suffix(modifications),
            replace_all,
            plural_suffix(replace_all)
        ));
        for (index, edit) in edits.iter().enumerate() {
            lines.extend(format_one_edit(
                index + 1,
                &edit.old_string,
                &edit.new_string,
                edit.replace_all,
                index == 0 && edit.old_string.is_empty(),
            ));
        }
    }
    if !is_empty_result(result) {
        lines.push(format!("  Result: {}", full_value_text(result)));
    }
    lines
}

fn format_write_tool_lines(result: &Value, arguments: Option<&Value>) -> Vec<String> {
    let mut lines = vec!["Tool result: write".to_string()];
    let path = result_path(result)
        .or_else(|| arguments.and_then(file_path_arg))
        .unwrap_or("unknown");
    lines.push(format!("  Writing file: {path}"));

    if let Some(args) = arguments {
        let mode = args
            .get("mode")
            .and_then(Value::as_str)
            .map_or("overwrite", write_mode_label);
        lines.push(format!("  Mode: {mode}"));
        let content = string_field(args, "content");
        if content.is_empty() {
            lines.push("  Edit #1: Empty file write".to_string());
            lines.push("    Empty file".to_string());
        } else {
            let operation = if args.get("mode").and_then(Value::as_str) == Some("a") {
                "Append content"
            } else {
                "File content"
            };
            lines.push(format!("  Edit #1: {operation}"));
            for line in preview_lines(content, 20) {
                lines.push(format!("    +{line}"));
            }
        }
    }

    if result.get("written").and_then(Value::as_bool) == Some(true) {
        lines.push("  Status: written".to_string());
    } else if !is_empty_result(result) {
        lines.push(format!("  Result: {}", value_preview(result)));
    }
    lines
}

fn write_mode_label(mode: &str) -> &'static str {
    match mode {
        "a" => "append",
        _ => "overwrite",
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct EditOperation {
    old_string: String,
    new_string: String,
    replace_all: bool,
}

fn edit_operations(args: &Value) -> Vec<EditOperation> {
    args.get("edits")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    item.as_object().map(|object| EditOperation {
                        old_string: object
                            .get("old_string")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        new_string: object
                            .get("new_string")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        replace_all: object
                            .get("replace_all")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn format_one_edit(
    index: usize,
    old_string: &str,
    new_string: &str,
    replace_all: bool,
    is_new_file: bool,
) -> Vec<String> {
    let operation_type = if is_new_file {
        "New file creation"
    } else {
        "Content modification"
    };
    let replace_suffix = if replace_all { " (replace all)" } else { "" };
    let mut lines = vec![format!("  Edit #{index}: {operation_type}{replace_suffix}")];
    if old_string.is_empty() {
        if is_new_file {
            if new_string.is_empty() {
                lines.push("    Empty file".to_string());
            } else {
                for line in preview_lines(new_string, 15) {
                    lines.push(format!("    +{line}"));
                }
            }
        } else {
            lines.push("    Empty match string".to_string());
            for line in preview_lines(new_string, 15) {
                lines.push(format!("    +{line}"));
            }
        }
    } else {
        lines.extend(unified_diff_lines(old_string, new_string, 18));
    }
    lines
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DiffLine<'a> {
    Text(&'a str),
    EofNewline,
}

fn unified_diff_lines(old_string: &str, new_string: &str, max_lines: usize) -> Vec<String> {
    if old_string == new_string {
        return vec!["    No changes detected".to_string()];
    }
    let old_lines = split_diff_lines(old_string);
    let new_lines = split_diff_lines(new_string);
    let old_len = old_lines.len();
    let new_len = new_lines.len();
    let mut prefix = 0usize;
    while prefix < old_len && prefix < new_len && old_lines[prefix] == new_lines[prefix] {
        prefix += 1;
    }

    let mut suffix = 0usize;
    while suffix < old_len.saturating_sub(prefix)
        && suffix < new_len.saturating_sub(prefix)
        && old_lines[old_len - suffix - 1] == new_lines[new_len - suffix - 1]
    {
        suffix += 1;
    }

    let old_change_end = old_len.saturating_sub(suffix);
    let new_change_end = new_len.saturating_sub(suffix);
    let context = 2usize;
    let old_context_start = prefix.saturating_sub(context);
    let new_context_start = prefix.saturating_sub(context);
    let old_after_end = (old_change_end + context).min(old_len);
    let new_after_end = (new_change_end + context).min(new_len);
    let old_start = old_context_start + 1;
    let new_start = new_context_start + 1;
    let old_span = old_after_end.saturating_sub(old_context_start);
    let new_span = new_after_end.saturating_sub(new_context_start);

    let mut lines = vec![format!(
        "    @@ -{old_start},{old_span} +{new_start},{new_span} @@"
    )];
    let mut truncated = false;
    if old_context_start > 0 || new_context_start > 0 {
        let omitted = old_context_start.max(new_context_start);
        push_diff_preview_line(
            &mut lines,
            &mut truncated,
            max_lines,
            format!("     ... ({omitted} unchanged lines before)"),
        );
    }
    for line in &old_lines[old_context_start..prefix] {
        push_diff_preview_line(
            &mut lines,
            &mut truncated,
            max_lines,
            format!("     {}", preview_diff_line(*line)),
        );
    }
    for line in &old_lines[prefix..old_change_end] {
        push_diff_preview_line(
            &mut lines,
            &mut truncated,
            max_lines,
            format!("    -{}", preview_diff_line(*line)),
        );
    }
    for line in &new_lines[prefix..new_change_end] {
        push_diff_preview_line(
            &mut lines,
            &mut truncated,
            max_lines,
            format!("    +{}", preview_diff_line(*line)),
        );
    }
    for line in &old_lines[old_change_end..old_after_end] {
        push_diff_preview_line(
            &mut lines,
            &mut truncated,
            max_lines,
            format!("     {}", preview_diff_line(*line)),
        );
    }
    if old_after_end < old_len || new_after_end < new_len {
        let omitted = old_len
            .saturating_sub(old_after_end)
            .max(new_len.saturating_sub(new_after_end));
        push_diff_preview_line(
            &mut lines,
            &mut truncated,
            max_lines,
            format!("     ... ({omitted} unchanged lines after)"),
        );
    }
    if truncated {
        if lines.len() >= max_lines.max(2) {
            lines.pop();
        }
        lines.push("    ... (diff truncated)".to_string());
    }
    lines
}

fn split_diff_lines(content: &str) -> Vec<DiffLine<'_>> {
    if content.is_empty() {
        return Vec::new();
    }
    let mut parts = content.split('\n').collect::<Vec<_>>();
    let has_eof_newline = content.ends_with('\n');
    if has_eof_newline {
        parts.pop();
    }
    let mut lines = parts.into_iter().map(DiffLine::Text).collect::<Vec<_>>();
    if has_eof_newline {
        lines.push(DiffLine::EofNewline);
    }
    lines
}

fn preview_diff_line(line: DiffLine<'_>) -> String {
    match line {
        DiffLine::Text("") => "<blank line>".to_string(),
        DiffLine::Text(line) => preview_line(line),
        DiffLine::EofNewline => "<EOF newline>".to_string(),
    }
}

fn push_diff_preview_line(
    lines: &mut Vec<String>,
    truncated: &mut bool,
    max_lines: usize,
    line: String,
) {
    if lines.len() < max_lines.max(2) {
        lines.push(line);
    } else {
        *truncated = true;
    }
}

fn format_view_tool_lines(result: &Value, arguments: Option<&Value>) -> Vec<String> {
    let mut lines = vec!["Tool result: view".to_string()];
    let path = result
        .get("file_path")
        .or_else(|| result.get("path"))
        .or_else(|| result.pointer("/metadata/file_path"))
        .or_else(|| arguments.and_then(|args| args.get("file_path")))
        .or_else(|| arguments.and_then(|args| args.get("path")))
        .and_then(Value::as_str);
    if let Some(path) = path {
        lines.push(format!("  Viewing file: {path}"));
    }

    let start_line = result
        .pointer("/metadata/current_segment/start_line")
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok());
    let end_line = result
        .pointer("/metadata/current_segment/end_line")
        .and_then(Value::as_u64);
    let total_lines = result
        .pointer("/metadata/total_lines")
        .and_then(Value::as_u64);
    if let (Some(start), Some(end), Some(total)) = (start_line, end_line, total_lines) {
        lines.push(format!("  Lines: {start}-{end} of {total}"));
    }

    if let Some(content) = result
        .get("content")
        .and_then(Value::as_str)
        .or_else(|| result.as_str())
    {
        lines.extend(format_view_content_lines(content, start_line));
    } else if let Some(message) = result
        .get("message")
        .or_else(|| result.get("error"))
        .and_then(Value::as_str)
    {
        lines.push(format!("  {message}"));
    } else {
        lines.push(format!("  {}", value_preview(result)));
    }
    if let Some(metadata) = result.get("metadata") {
        if let Some(truncation) = metadata.get("truncation_info") {
            lines.push(format!("  Truncation: {}", value_preview(truncation)));
        }
    }
    lines
}

fn format_view_content_lines(content: &str, start_line: Option<usize>) -> Vec<String> {
    if content.is_empty() {
        return vec!["    Empty file".to_string()];
    }
    let preview = preview_lines(content, 20);
    let line_number_width = start_line.map_or(0, |start| {
        start.saturating_add(preview.len()).to_string().len().max(4)
    });
    preview
        .into_iter()
        .enumerate()
        .map(|(index, line)| {
            if line.starts_with("... (") {
                return format!("    {line}");
            }
            start_line.map_or_else(
                || format!("    │ {line}"),
                |start| {
                    format!(
                        "    {:>line_number_width$} │ {line}",
                        start.saturating_add(index)
                    )
                },
            )
        })
        .collect()
}

fn format_summarize_tool_lines(result: &Value, arguments: Option<&Value>) -> Vec<String> {
    let payload = result.get("payload").unwrap_or(result);
    let content = payload_string(
        payload,
        &["content", "handoff_content", "summary_markdown", "summary"],
    )
    .or_else(|| arguments.and_then(|args| payload_string(args, &["content", "summary"])))
    .unwrap_or_default();
    let auto_load_files = payload_string_array(payload, "auto_load_files")
        .or_else(|| arguments.and_then(|args| payload_string_array(args, "auto_load_files")))
        .unwrap_or_default();

    let mut lines = vec!["Tool result: summarize".to_string()];
    lines.push("  Summary: Progress summarized, continuing with fresh context".to_string());
    if !content.trim().is_empty() {
        for line in preview_lines(&content, 12) {
            lines.push(format!("    │ {line}"));
        }
    }
    if !auto_load_files.is_empty() {
        lines.push(format!(
            "  Auto-load files: {}",
            auto_load_files
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    lines
}

fn format_custom_context_event_lines(kind: &str, payload: &Value) -> Option<Vec<String>> {
    let normalized = normalized_event_kind(kind);
    let context_payload = payload.get("payload").unwrap_or(payload);
    if event_kind_matches(
        &normalized,
        &[
            "compact_start",
            "compact_started",
            "compaction_start",
            "compaction_started",
        ],
    ) {
        return Some(format_compaction_started_lines(context_payload));
    }
    if event_kind_matches(
        &normalized,
        &[
            "compact_complete",
            "compact_completed",
            "compaction_complete",
            "compaction_completed",
        ],
    ) {
        return Some(format_compaction_completed_lines(context_payload, payload));
    }
    if event_kind_matches(
        &normalized,
        &[
            "compact_failed",
            "compact_failure",
            "compaction_failed",
            "compaction_failure",
        ],
    ) {
        return Some(vec![format!(
            "Compact failed: {}",
            compact_error_text(context_payload, payload)
        )]);
    }
    if event_kind_matches(
        &normalized,
        &[
            "handoff_start",
            "handoff_started",
            "summary_start",
            "summary_started",
        ],
    ) {
        return Some(format_handoff_started_lines(context_payload));
    }
    if event_kind_matches(
        &normalized,
        &[
            "handoff_complete",
            "handoff_completed",
            "summary_complete",
            "summary_completed",
        ],
    ) {
        return Some(format_handoff_completed_lines(context_payload, payload));
    }
    if event_kind_matches(
        &normalized,
        &[
            "handoff_failed",
            "handoff_failure",
            "summary_failed",
            "summary_failure",
        ],
    ) {
        return Some(vec![format!(
            "Summary failed: {}",
            compact_error_text(context_payload, payload)
        )]);
    }
    None
}

fn normalized_event_kind(kind: &str) -> String {
    kind.to_ascii_lowercase().replace(['.', '-'], "_")
}

fn event_kind_matches(normalized: &str, candidates: &[&str]) -> bool {
    candidates
        .iter()
        .any(|candidate| normalized == *candidate || normalized.ends_with(&format!("_{candidate}")))
}

fn format_compaction_started_lines(payload: &Value) -> Vec<String> {
    let message_count = payload_u64(
        payload,
        &[
            "message_count",
            "messages",
            "original_count",
            "original_message_count",
            "original_messages",
        ],
    );
    vec![message_count.map_or_else(
        || "Context compacting...".to_string(),
        |count| format!("Context compacting {count} messages..."),
    )]
}

fn format_compaction_completed_lines(payload: &Value, wrapper: &Value) -> Vec<String> {
    let mut lines = vec!["Context compacted".to_string()];
    let original = payload_u64(
        payload,
        &[
            "original_count",
            "original_message_count",
            "original_messages",
        ],
    );
    let compacted = payload_u64(
        payload,
        &[
            "compacted_count",
            "compacted_message_count",
            "compacted_messages",
            "message_count",
            "messages",
        ],
    );
    match (original, compacted) {
        (Some(original), Some(compacted)) if original > 0 => {
            let reduction = original
                .saturating_sub(compacted)
                .saturating_mul(100)
                .saturating_add(original / 2)
                / original;
            lines.push(format!(
                "  Summary: {original} -> {compacted} messages ({reduction}% reduction)"
            ));
        }
        (_, Some(compacted)) => {
            if let Some(revision) = payload_u64(payload, &["revision"]) {
                lines.push(format!(
                    "  Summary: revision {revision}, {compacted} messages retained"
                ));
            } else {
                lines.push(format!("  Summary: {compacted} messages retained"));
            }
        }
        _ => {
            let preview = payload_string(wrapper, &["preview"])
                .unwrap_or_else(|| "Context compaction completed".to_string());
            lines.push(format!("  Summary: {}", preview_line(&preview)));
        }
    }
    if let Some(summary) = payload_string(
        payload,
        &["summary", "summary_markdown", "content", "handoff_content"],
    ) {
        push_indented_preview(&mut lines, &summary, 8);
    }
    lines
}

fn format_handoff_started_lines(payload: &Value) -> Vec<String> {
    let message_count = payload_u64(
        payload,
        &[
            "message_count",
            "messages",
            "original_count",
            "original_message_count",
        ],
    );
    vec![message_count.map_or_else(
        || "Summarizing progress...".to_string(),
        |count| format!("Summarizing progress ({count} messages)..."),
    )]
}

fn format_handoff_completed_lines(payload: &Value, wrapper: &Value) -> Vec<String> {
    let mut lines = vec!["Summary complete".to_string()];
    lines.push("  Summary: Progress summarized, continuing with fresh context".to_string());
    let content = payload_string(
        payload,
        &["handoff_content", "content", "summary_markdown", "summary"],
    )
    .or_else(|| payload_string(wrapper, &["preview"]));
    if let Some(content) = content {
        push_indented_preview(&mut lines, &content, 12);
    }
    lines
}

fn push_indented_preview(lines: &mut Vec<String>, content: &str, max_lines: usize) {
    if content.trim().is_empty() {
        return;
    }
    for line in preview_lines(content, max_lines) {
        lines.push(format!("    │ {line}"));
    }
}

fn compact_error_text(payload: &Value, wrapper: &Value) -> String {
    payload_string(payload, &["error", "message", "reason"])
        .or_else(|| payload_string(wrapper, &["error", "message", "preview"]))
        .unwrap_or_else(|| "unknown error".to_string())
}

fn payload_u64(value: &Value, keys: &[&str]) -> Option<u64> {
    keys.iter().find_map(|key| {
        value.get(*key).and_then(|item| {
            item.as_u64()
                .or_else(|| item.as_i64().and_then(|number| u64::try_from(number).ok()))
                .or_else(|| item.as_str().and_then(|text| text.parse::<u64>().ok()))
        })
    })
}

fn payload_string(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        let item = value.get(*key)?;
        match item {
            Value::String(text) if !text.trim().is_empty() => Some(text.clone()),
            Value::Null => None,
            other if !other.to_string().trim().is_empty() => Some(other.to_string()),
            _ => None,
        }
    })
}

fn payload_string_array(value: &Value, key: &str) -> Option<Vec<String>> {
    let item = value.get(key)?;
    let values = match item {
        Value::Array(items) => items
            .iter()
            .filter_map(|item| item.as_str().map(str::to_string))
            .collect::<Vec<_>>(),
        Value::String(text) if !text.trim().is_empty() => vec![text.clone()],
        _ => Vec::new(),
    };
    (!values.is_empty()).then_some(values)
}

fn is_task_tool_name(name: &str) -> bool {
    matches!(
        name,
        "task_create" | "task_get" | "task_update" | "task_list"
    )
}

fn format_task_tool_lines(name: &str, structured: &Value, display_value: &Value) -> Vec<String> {
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

fn is_task_snapshot_event(kind: &str) -> bool {
    let normalized = kind.to_ascii_lowercase().replace(['.', '-'], "_");
    normalized == TASK_SNAPSHOT_EVENT_KIND
        || normalized == "task_panel"
        || normalized.ends_with("_task_snapshot")
        || normalized.ends_with("_task_panel")
}

fn task_panel_items_from_value(value: &Value) -> Option<Vec<TaskPanelItem>> {
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

#[allow(clippy::cast_precision_loss)]
fn tool_duration_label(metadata: &serde_json::Map<String, Value>) -> Option<String> {
    let millis = metadata.get("duration_ms").and_then(Value::as_u64)?;
    if millis < 1_000 {
        Some(format!("{millis}ms"))
    } else {
        Some(format!("{:.2}s", millis as f64 / 1_000.0))
    }
}

fn result_path(value: &Value) -> Option<&str> {
    file_path_arg(value)
}

fn file_path_arg(value: &Value) -> Option<&str> {
    value
        .get("file_path")
        .or_else(|| value.get("path"))
        .and_then(Value::as_str)
}

fn edit_result_status(value: &Value) -> Option<&'static str> {
    if value.get("created").and_then(Value::as_bool) == Some(true) {
        Some("created")
    } else if value.get("edited").and_then(Value::as_bool) == Some(true) {
        Some("edited")
    } else {
        None
    }
}

fn string_field<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or_default()
}

fn previous_char_boundary(text: &str, index: usize) -> usize {
    let mut index = index.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index = index.saturating_sub(1);
    }
    index
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
    if count == 1 {
        ""
    } else {
        "s"
    }
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

fn streaming_tool_state_is_available(state: &StreamingToolCallState, key: &str) -> bool {
    state.line_index.is_some()
        && state
            .linked_call_key
            .as_deref()
            .map_or(true, |linked| linked == key)
}

fn streaming_tool_arguments_match(
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

fn body_line_display_text(line: &str) -> &str {
    line.strip_prefix(ASSISTANT_CONTENT_PREFIX)
        .unwrap_or(line)
        .trim()
}

fn compact_status_text(text: &str, max_chars: usize) -> String {
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
