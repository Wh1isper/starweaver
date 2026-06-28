use std::{
    collections::{BTreeMap, HashMap, HashSet},
    env,
    path::Path,
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use starweaver_core::SessionId;
use starweaver_model::{PartDelta, StreamDelta};
use starweaver_runtime::{AgentStreamEvent, AgentStreamRecord, ModelResponseStreamEvent};
use starweaver_usage::{Usage, UsageSnapshot};

const DEFAULT_CONTEXT_WINDOW_TOKENS: u64 = 200_000;
const SHELL_OUTPUT_MAX_LINES: usize = 200;
const SHELL_STREAM_PREVIEW_MAX_LINES: usize = 6;
const TOOL_PREVIEW_MAX_CHARS: usize = 240;
pub(super) const COMPOSER_VISIBLE_LINES: usize = 5;

use crate::{
    prompt_input::{format_size_bytes, PromptAttachment, PromptInput},
    slash_commands::SlashCommandDefinition,
};

use super::{render::snapshot_interactive_lines, snapshot::TuiSnapshot};

mod commands;
mod composer;
mod cost;
mod formatting;
mod pickers;
mod streaming;

pub(super) use formatting::display_lines_for_stream_record;
use formatting::{
    append_delta_segments, assistant_content_line, body_line_display_text, cache_hit_rate_label,
    compact_status_text, format_custom_context_event_lines, format_streaming_tool_call_line,
    format_subagent_finished_line, format_subagent_running_line, format_tool_call_line,
    format_tool_return_lines, format_u64_with_commas, is_assistant_content_line,
    is_subagent_lifecycle_event_kind, is_subagent_start_event_kind, is_task_snapshot_event,
    is_task_tool_name, is_thinking_quote_line, merge_stream_fragment, model_choice_config_suffix,
    model_choice_label, normalized_event_kind, pasted_image_paths, previous_char_boundary,
    push_shell_output_lines, push_usage_entry_lines, push_user_prompt_lines, streaming_part_kind,
    streaming_tool_arguments_match, streaming_tool_state_is_available, subagent_display_id,
    task_panel_items_from_value, tool_call_visibility_key,
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
pub(super) struct SubagentDisplayState {
    pub(super) line_index: usize,
    pub(super) tool_names: Vec<String>,
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
    /// Current durable session id, when one exists.
    pub session_id: Option<String>,
    /// Stable provider-routing affinity id for this TUI process.
    pub session_affinity_id: String,
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
    /// Latest model stream transport diagnostic shown in the status bar.
    pub(super) model_transport_status: Option<String>,
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
    pub(super) subagent_states: HashMap<String, SubagentDisplayState>,
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
    pub(super) cancel_requested: bool,
    pub(super) footer_mode: FooterMode,
    pub(super) goal_task: Option<String>,
    pub(super) goal_active: bool,
    pub(super) goal_iteration: usize,
    pub(super) goal_max_iterations: usize,
    pending_goal_submission: Option<String>,
    pub(super) context_tokens: Option<u64>,
    pub(super) latest_request_total_tokens: Option<u64>,
    pub(super) current_run_id: Option<String>,
    pub(super) current_run_usage: Option<Usage>,
    pub(super) context_window: Option<u64>,
    usage_snapshots: BTreeMap<String, UsageSnapshot>,
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
            session_affinity_id: SessionId::new().as_str().to_string(),
            body: Vec::new(),
            status: "IDLE".to_string(),
            input: String::new(),
            input_cursor: 0,
            input_cursor_input_len: 0,
            profile: "general".to_string(),
            model: "local_echo".to_string(),
            phase: "ready".to_string(),
            model_transport_status: None,
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
            subagent_states: HashMap::new(),
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
            cancel_requested: false,
            footer_mode: FooterMode::Context,
            goal_task: None,
            goal_active: false,
            goal_iteration: 0,
            goal_max_iterations: 10,
            pending_goal_submission: None,
            context_tokens: None,
            latest_request_total_tokens: None,
            current_run_id: None,
            current_run_usage: None,
            context_window: Some(DEFAULT_CONTEXT_WINDOW_TOKENS),
            usage_snapshots: BTreeMap::new(),
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

    /// Set the maximum runtime goal retry iterations shown by `/goal`.
    pub fn set_goal_max_iterations(&mut self, max_iterations: usize) {
        self.goal_max_iterations = max_iterations.max(1);
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
        self.model_transport_status = None;
        self.streaming_parts.clear();
        self.streaming_text_seen = false;
        self.streaming_reasoning_seen = false;
        self.visible_text_seen = false;
        self.streaming_tool_calls.clear();
        self.visible_tool_calls.clear();
        self.tool_call_arguments.clear();
        self.subagent_states.clear();
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
        self.current_run_id = None;
        self.current_run_usage = None;
        self.model_transport_status = None;
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
        self.subagent_states.clear();
        self.pending_hitl = None;
        self.pending_submission_display_prompt = None;
        self.model_picker_open = false;
        self.session_picker_open = false;
        self.finish_active_goal_without_runtime_event("unverified_stop");
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
        self.subagent_states.clear();
        self.pending_hitl = None;
        self.pending_submission_display_prompt = None;
        self.model_picker_open = false;
        self.session_picker_open = false;
        self.finish_active_goal_without_runtime_event("error");
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
        self.subagent_states.clear();
        self.pending_hitl = None;
        self.pending_submission_display_prompt = None;
        self.model_picker_open = false;
        self.session_picker_open = false;
        self.finish_active_goal_without_runtime_event("cancelled");
        self.body.push(format!("Run cancelled: {reason}"));
    }

    pub(super) fn take_pending_clear_context(&mut self) -> bool {
        std::mem::take(&mut self.pending_clear_context)
    }

    pub(crate) fn clear_context_view(&mut self) {
        self.session_id = None;
        self.body.clear();
        self.context_tokens = None;
        self.latest_request_total_tokens = None;
        self.model_transport_status = None;
        self.current_run_id = None;
        self.current_run_usage = None;
        self.usage_snapshots.clear();
        self.streaming_parts.clear();
        self.streaming_text_seen = false;
        self.streaming_reasoning_seen = false;
        self.visible_text_seen = false;
        self.streaming_tool_calls.clear();
        self.visible_tool_calls.clear();
        self.tool_call_arguments.clear();
        self.subagent_states.clear();
        self.pending_hitl = None;
        self.task_panel_items.clear();
        self.goal_task = None;
        self.goal_active = false;
        self.pending_goal_submission = None;
        self.phase = "cleared".to_string();
        self.status = "IDLE".to_string();
        self.scroll_to_bottom();
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
            .is_none_or(|index| index >= self.body.len())
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

    pub(super) const fn pending_hitl(&self) -> Option<&HitlPanelState> {
        self.pending_hitl.as_ref()
    }

    pub(super) fn task_panel_items(&self) -> &[TaskPanelItem] {
        &self.task_panel_items
    }

    fn record_steering_message(&mut self, text: String) -> SteeringSubmission {
        let id = format!("steer_{}", self.next_steering_id);
        self.next_steering_id = self.next_steering_id.saturating_add(1);
        self.body.push(format!("Steering: {text}"));
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

    pub(crate) fn take_pending_goal_submission(&mut self) -> Option<(String, usize)> {
        self.pending_goal_submission
            .take()
            .map(|objective| (objective, self.goal_max_iterations.max(1)))
    }

    pub(super) fn apply_goal_event_payload(&mut self, kind: &str, payload: &Value) {
        let normalized = kind.to_ascii_lowercase().replace(['.', '-'], "_");
        if normalized == "goal_iteration" || normalized.ends_with("_goal_iteration") {
            self.goal_active = true;
            if let Some(task) = payload.get("task").and_then(Value::as_str) {
                self.goal_task = Some(task.to_string());
            }
            if let Some(iteration) = payload.get("iteration").and_then(Value::as_u64) {
                self.goal_iteration = usize::try_from(iteration).unwrap_or(usize::MAX);
            }
            if let Some(max_iterations) = payload.get("max_iterations").and_then(Value::as_u64) {
                self.goal_max_iterations =
                    usize::try_from(max_iterations).unwrap_or(usize::MAX).max(1);
            }
        } else if normalized == "goal_complete"
            || normalized == "goal_completed"
            || normalized.ends_with("_goal_complete")
            || normalized.ends_with("_goal_completed")
        {
            if let Some(iteration) = payload.get("iteration").and_then(Value::as_u64) {
                self.goal_iteration = usize::try_from(iteration).unwrap_or(usize::MAX);
            }
            if let Some(max_iterations) = payload.get("max_iterations").and_then(Value::as_u64) {
                self.goal_max_iterations =
                    usize::try_from(max_iterations).unwrap_or(usize::MAX).max(1);
            }
            self.goal_active = false;
        }
    }

    pub(super) fn push_goal_total_tokens_report(&mut self) {
        let usage = self.current_run_usage.clone().unwrap_or_default();
        self.body.push(format!(
            "[Goal] Total tokens: {} (input: {}, cache read: {}, cache write: {}, output: {})",
            format_u64_with_commas(usage.total_tokens),
            format_u64_with_commas(usage.input_tokens),
            format_u64_with_commas(usage.cache_read_tokens),
            format_u64_with_commas(usage.cache_write_tokens),
            format_u64_with_commas(usage.output_tokens)
        ));
    }

    fn finish_active_goal_without_runtime_event(&mut self, reason: &str) {
        if self.goal_active {
            self.goal_active = false;
            self.body.push(format!("[Goal] Completed: {reason}"));
            self.push_goal_total_tokens_report();
        }
        self.pending_goal_submission = None;
    }
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
