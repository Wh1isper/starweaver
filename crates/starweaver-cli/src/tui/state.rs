use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    env,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use starweaver_agent::{
    CLARIFYING_QUESTIONS_REQUEST_KIND, ClarifyingQuestion, ClarifyingQuestionAnswers,
};
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
    args::TuiRenderMode,
    prompt_input::{PromptAttachment, PromptInput, format_size_bytes},
    slash_commands::SlashCommandDefinition,
};

use super::{
    render::snapshot_interactive_lines,
    snapshot::TuiSnapshot,
    timeline::{
        ContextEventCategory, NoticeLevel, SubagentStatus, SubagentTimelineItem, SubagentUpdate,
        ToolActivityStatus, ToolConciseSummary, ToolSummaryCategory, ToolSummaryImportance,
        ToolTimelineItem, ToolVisibility, TuiItemId, TuiProjection, TuiTimeline, project_timeline,
        value_args_preview,
    },
};

mod command_palette;
mod commands;
mod composer;
mod cost;
mod formatting;
mod history;
mod pickers;
mod streaming;
mod tasks;

pub(in crate::tui) use command_palette::{CommandPaletteAccept, CommandPaletteState};
pub(super) use formatting::display_lines_for_stream_record;
use formatting::{
    body_line_display_text, cache_hit_rate_label, compact_status_text,
    format_custom_context_event_lines, format_streaming_tool_call_line, format_tool_call_line,
    format_tool_return_lines, format_u64_with_commas, is_subagent_lifecycle_event_kind,
    is_subagent_start_event_kind, is_task_snapshot_event, is_task_tool_name, merge_stream_fragment,
    model_choice_config_suffix, model_choice_label, normalized_event_kind, pasted_image_paths,
    push_shell_output_lines, push_usage_entry_lines, streaming_part_kind,
    streaming_tool_arguments_match, streaming_tool_state_is_available, subagent_display_id,
    task_panel_items_from_value, tool_call_visibility_key,
};
pub(in crate::tui) use history::HistorySearchState;
use history::load_prompt_history;

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ActiveModelSegmentKind {
    Text,
    Thinking,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ActiveModelSegment {
    item_id: TuiItemId,
    kind: ActiveModelSegmentKind,
    part_index: Option<usize>,
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
    item_id: Option<TuiItemId>,
    linked_call_key: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SubagentDisplayState {
    pub(super) item_id: TuiItemId,
    pub(super) agent_name: String,
    pub(super) status: String,
    pub(super) tool_names: Vec<String>,
    pub(super) output_preview: String,
    pub(super) output_markdown: String,
    pub(super) request_count: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct HitlPanelState {
    pub(super) approval_id: Option<String>,
    pub(super) tool_call_id: String,
    pub(super) tool_name: String,
    pub(super) request_preview: Option<String>,
    pub(super) clarifying: Option<ClarifyingQuestionUiState>,
    pub(super) command: Option<String>,
    pub(super) risk_level: Option<String>,
    pub(super) reason: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ClarifyingQuestionUiState {
    pub(super) questions: Vec<ClarifyingQuestion>,
    pub(super) question_index: usize,
    pub(super) option_index: usize,
    pub(super) selections: Vec<BTreeSet<usize>>,
    pub(super) free_form_answers: BTreeMap<usize, String>,
    pub(super) free_form_active: bool,
}

impl ClarifyingQuestionUiState {
    fn from_request(request: &Value) -> Option<Self> {
        if request.get("kind").and_then(Value::as_str) != Some(CLARIFYING_QUESTIONS_REQUEST_KIND) {
            return None;
        }
        let questions =
            serde_json::from_value::<Vec<ClarifyingQuestion>>(request.get("questions")?.clone())
                .ok()?;
        if questions.is_empty() {
            return None;
        }
        let selections = vec![BTreeSet::new(); questions.len()];
        Some(Self {
            questions,
            question_index: 0,
            option_index: 0,
            selections,
            free_form_answers: BTreeMap::new(),
            free_form_active: false,
        })
    }

    pub(super) fn current_question(&self) -> Option<&ClarifyingQuestion> {
        self.questions.get(self.question_index)
    }

    fn selected_answer(&self, index: usize) -> Option<String> {
        if let Some(answer) = self.free_form_answers.get(&index) {
            return Some(answer.clone());
        }
        let question = self.questions.get(index)?;
        let labels = self
            .selections
            .get(index)?
            .iter()
            .filter_map(|option| question.options.get(*option))
            .map(|option| option.label.clone())
            .collect::<Vec<_>>();
        (!labels.is_empty()).then(|| labels.join(", "))
    }

    fn answers(&self) -> Option<ClarifyingQuestionAnswers> {
        let mut answers = BTreeMap::new();
        for (index, question) in self.questions.iter().enumerate() {
            answers.insert(question.question.clone(), self.selected_answer(index)?);
        }
        Some(ClarifyingQuestionAnswers {
            answers,
            response: None,
        })
    }
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
    /// Frozen elapsed time for the current or most recently completed run.
    pub(super) session_elapsed_frozen: Duration,
    /// Monotonic start time while the current run is active.
    pub(super) session_timer_started_at: Option<Instant>,
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
    /// Last rendered composer content width, used for visual-line cursor movement.
    pub(super) composer_content_width: usize,
    /// Desired display column retained across consecutive vertical cursor moves.
    pub(super) composer_preferred_column: Option<usize>,
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
    /// Scrollback offset from bottom.
    pub scroll_offset: usize,
    /// Last rendered transcript height, used to keep scroll handling cheap between frames.
    pub(super) rendered_body_len: usize,
    /// Last rendered body viewport height, used to keep scroll handling cheap between frames.
    pub(super) body_viewport_height: usize,
    /// Number of changed output lines received while transcript following is paused.
    pub(super) unread_output_lines: usize,
    /// Multiline composer scrollback offset from the bottom of the draft.
    pub(super) input_scroll_offset: usize,
    /// Short-lived composer status for paste, media attach, and steering actions.
    pub(super) input_status: Option<String>,
    /// Image attachments queued into the fixed composer.
    pub(super) pending_attachments: Vec<PromptAttachment>,
    restored_prompt_parts: Option<(Vec<String>, Vec<String>)>,
    pub(super) history: Vec<String>,
    history_path: PathBuf,
    history_persistence_enabled: bool,
    pub(super) history_index: Option<usize>,
    pub(super) history_draft: String,
    history_search: Option<HistorySearchState>,
    streaming_parts: HashMap<usize, StreamingPartKind>,
    active_model_segment: Option<ActiveModelSegment>,
    timeline: TuiTimeline,
    timeline_projection: TuiProjection,
    projection_batch_depth: usize,
    projection_dirty: bool,
    render_mode: TuiRenderMode,
    streaming_text_seen: bool,
    streaming_reasoning_seen: bool,
    visible_text_seen: bool,
    streaming_tool_calls: HashMap<usize, StreamingToolCallState>,
    visible_tool_calls: HashSet<String>,
    tool_items_by_call_id: HashMap<String, TuiItemId>,
    tool_items_by_key: HashMap<String, TuiItemId>,
    tool_call_arguments: HashMap<String, Value>,
    pub(super) subagent_states: HashMap<String, SubagentDisplayState>,
    pending_hitl: Option<HitlPanelState>,
    hitl_reload_session_id: Option<String>,
    task_panel_items: Vec<TaskPanelItem>,
    task_panel_open: bool,
    task_panel_index: usize,
    task_panel_completed_hidden: bool,
    pending_clear_context: bool,
    selection_mode: bool,
    selection_index: Option<usize>,
    pending_submission_display_prompt: Option<String>,
    custom_commands: BTreeMap<String, SlashCommandDefinition>,
    skills: Vec<crate::profiles::SkillSummary>,
    command_palette: Option<CommandPaletteState>,
    command_palette_dismissed_input: Option<String>,
    help_panel_open: bool,
    model_choices: Vec<ModelChoice>,
    model_picker_open: bool,
    model_picker_index: usize,
    session_choices: Vec<SessionChoice>,
    session_picker_open: bool,
    session_picker_index: usize,
    pending_session_command: Option<String>,
    pending_shell_command: Option<String>,
    pub(super) shell_running: bool,
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
        let workspace_dir =
            env::current_dir().map_or_else(|_| ".".to_string(), |path| path.display().to_string());
        let (history_path, history) = load_prompt_history(config_dir, &workspace_dir);
        Self {
            config_dir: config_dir.display().to_string(),
            workspace_dir,
            session_id: None,
            session_affinity_id: SessionId::new().as_str().to_string(),
            session_elapsed_frozen: Duration::ZERO,
            session_timer_started_at: None,
            body: Vec::new(),
            status: "IDLE".to_string(),
            input: String::new(),
            input_cursor: 0,
            input_cursor_input_len: 0,
            composer_content_width: 78,
            composer_preferred_column: None,
            profile: "general".to_string(),
            model: "local_echo".to_string(),
            phase: "ready".to_string(),
            model_transport_status: None,
            running: false,
            scroll_offset: usize::MAX,
            rendered_body_len: 0,
            body_viewport_height: 1,
            unread_output_lines: 0,
            input_scroll_offset: 0,
            input_status: None,
            pending_attachments: Vec::new(),
            restored_prompt_parts: None,
            history,
            history_path,
            history_persistence_enabled: true,
            history_index: None,
            history_draft: String::new(),
            history_search: None,
            streaming_parts: HashMap::new(),
            active_model_segment: None,
            timeline: TuiTimeline::default(),
            timeline_projection: TuiProjection::default(),
            projection_batch_depth: 0,
            projection_dirty: false,
            render_mode: TuiRenderMode::Normal,
            streaming_text_seen: false,
            streaming_reasoning_seen: false,
            visible_text_seen: false,
            streaming_tool_calls: HashMap::new(),
            visible_tool_calls: HashSet::new(),
            tool_items_by_call_id: HashMap::new(),
            tool_items_by_key: HashMap::new(),
            tool_call_arguments: HashMap::new(),
            subagent_states: HashMap::new(),
            pending_hitl: None,
            hitl_reload_session_id: None,
            task_panel_items: Vec::new(),
            task_panel_open: false,
            task_panel_index: 0,
            task_panel_completed_hidden: false,
            pending_clear_context: false,
            selection_mode: false,
            selection_index: None,
            pending_submission_display_prompt: None,
            custom_commands: BTreeMap::new(),
            skills: Vec::new(),
            command_palette: None,
            command_palette_dismissed_input: None,
            help_panel_open: false,
            model_choices: Vec::new(),
            model_picker_open: false,
            model_picker_index: 0,
            session_choices: Vec::new(),
            session_picker_open: false,
            session_picker_index: 0,
            pending_session_command: None,
            pending_shell_command: None,
            shell_running: false,
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

    #[cfg(test)]
    pub(super) fn welcome_ephemeral(config_dir: &Path) -> Self {
        let mut state = Self::welcome(config_dir);
        state.history.clear();
        state.history_persistence_enabled = false;
        state
    }

    pub(crate) fn set_render_mode(&mut self, render_mode: TuiRenderMode) {
        if self.render_mode == render_mode {
            self.input_status = Some(format!("display: {}", render_mode_label(render_mode)));
            return;
        }
        self.render_mode = render_mode;
        self.selection_mode = false;
        self.selection_index = None;
        self.input_status = Some(format!("display: {}", render_mode_label(render_mode)));
        self.reproject_body();
        self.scroll_to_bottom();
    }

    pub(crate) const fn render_mode(&self) -> TuiRenderMode {
        self.render_mode
    }

    pub(super) const fn timeline_generation(&self) -> u64 {
        self.timeline.generation()
    }

    pub(super) fn active_tool_label(&self) -> Option<String> {
        self.timeline_projection.active_tool_label()
    }

    pub(crate) const fn begin_projection_batch(&mut self) {
        self.projection_batch_depth = self.projection_batch_depth.saturating_add(1);
    }

    pub(crate) fn end_projection_batch(&mut self) {
        self.projection_batch_depth = self.projection_batch_depth.saturating_sub(1);
        if self.projection_batch_depth == 0 && self.projection_dirty {
            self.reproject_body();
        }
    }

    fn reproject_body(&mut self) {
        if self.projection_batch_depth > 0 {
            self.projection_dirty = true;
            return;
        }
        self.projection_dirty = false;
        let projection = project_timeline(&self.timeline, self.render_mode);
        let changed_while_paused = !self.is_at_bottom() && projection.lines != self.body;
        if changed_while_paused {
            let changed_lines = projection.lines.len().abs_diff(self.body.len()).max(1);
            self.unread_output_lines = self.unread_output_lines.saturating_add(changed_lines);
        }
        self.body.clone_from(&projection.lines);
        self.timeline_projection = projection;
        if self
            .selection_index
            .is_some_and(|index| index >= self.body.len())
        {
            self.selection_index = self.last_selectable_body_index();
        }
    }

    fn push_system_notice(&mut self, level: NoticeLevel, line: impl Into<String>) {
        self.finish_current_model_item();
        self.timeline.push_system_lines(level, vec![line.into()]);
        self.reproject_body();
    }

    pub(crate) fn push_transcript_notice(&mut self, line: impl Into<String>) {
        self.push_system_notice(NoticeLevel::Info, line);
    }

    pub(super) fn push_transcript_lines(&mut self, lines: Vec<String>) {
        self.finish_current_model_item();
        self.timeline.push_system_lines(NoticeLevel::Info, lines);
        self.reproject_body();
    }

    pub(crate) fn push_run_status_line(&mut self, line: impl Into<String>) {
        self.finish_current_model_item();
        self.timeline.push_run_status(line);
        self.reproject_body();
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

    /// Return elapsed wall-clock time for the current or most recently completed run.
    pub(crate) fn session_elapsed(&self) -> Duration {
        self.session_timer_started_at
            .map_or(self.session_elapsed_frozen, |started_at| {
                started_at.elapsed()
            })
    }

    pub(in crate::tui) fn session_elapsed_label(&self) -> String {
        format_session_duration(self.session_elapsed())
    }

    fn start_session_timer(&mut self) {
        self.session_elapsed_frozen = Duration::ZERO;
        self.session_timer_started_at = Some(Instant::now());
    }

    fn stop_session_timer(&mut self) {
        if let Some(started_at) = self.session_timer_started_at.take() {
            self.session_elapsed_frozen = started_at.elapsed();
        }
    }

    fn reset_session_metrics(&mut self) {
        self.session_elapsed_frozen = Duration::ZERO;
        self.session_timer_started_at = None;
        self.context_tokens = None;
        self.latest_request_total_tokens = None;
        self.current_run_id = None;
        self.current_run_usage = None;
        self.usage_snapshots.clear();
    }

    /// Set the maximum runtime goal retry iterations shown by `/goal`.
    pub fn set_goal_max_iterations(&mut self, max_iterations: usize) {
        self.goal_max_iterations = max_iterations.max(1);
    }

    /// Set model choices shown by `/model`.
    pub fn set_model_choices(&mut self, choices: Vec<ModelChoice>) {
        self.model_choices = choices;
        self.sync_model_picker_index_to_current();
        self.refresh_command_palette();
    }

    /// Return configured model choices.
    pub fn model_choices(&self) -> &[ModelChoice] {
        &self.model_choices
    }

    /// Set session choices shown by `/session`.
    pub fn set_session_choices(&mut self, choices: Vec<SessionChoice>) {
        self.session_choices = choices;
        self.sync_session_picker_index_to_current();
        self.refresh_command_palette();
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
        let session_changed = self.session_id.as_deref() != Some(snapshot.session_id.as_str());
        if session_changed {
            self.reset_session_metrics();
            self.reset_task_panel_for_session();
        }
        self.session_id = Some(snapshot.session_id.clone());
        let lines = snapshot_interactive_lines(snapshot);
        self.finish_current_model_item();
        self.active_model_segment = None;
        self.timeline.clear();
        self.timeline.push_system_lines(NoticeLevel::Info, lines);
        self.reproject_body();
        self.footer_mode = FooterMode::Context;
        self.input_status = None;
        self.pending_attachments.clear();
        self.restored_prompt_parts = None;
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
        self.pending_hitl = None;
        self.hitl_reload_session_id = None;
        self.set_task_panel_items(snapshot.tasks.clone());
        self.sync_session_picker_index_to_current();
        self.phase = "replay".to_string();
    }

    /// Begin rendering one submitted prompt.
    pub fn begin_run(&mut self, prompt: &str) {
        self.prepare_run_rendering();
        self.timeline.push_user_prompt(prompt);
        self.reproject_body();
    }

    pub(crate) fn begin_background_continuation(&mut self) {
        self.prepare_run_rendering();
        self.timeline.push_system_lines(
            NoticeLevel::Info,
            vec!["Background subagent result received; continuing session.".to_string()],
        );
        self.reproject_body();
    }

    fn prepare_run_rendering(&mut self) {
        self.start_session_timer();
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
        self.tool_items_by_call_id.clear();
        self.tool_items_by_key.clear();
        self.tool_call_arguments.clear();
        self.subagent_states.clear();
        self.pending_hitl = None;
        self.hitl_reload_session_id = None;
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
        self.finish_current_model_item();
        self.active_model_segment = None;
    }

    /// Mark a run as durably waiting while retaining live HITL and deferred-tool context.
    pub fn wait_run(&mut self, session_id: Option<String>) {
        self.stop_session_timer();
        if let Some(session_id) = session_id {
            self.session_id = Some(session_id);
        }
        self.running = false;
        self.cancel_requested = false;
        self.status = "WAITING".to_string();
        self.phase = "waiting".to_string();
        self.streaming_parts.clear();
        self.streaming_tool_calls.clear();
        self.pending_submission_display_prompt = None;
        self.model_picker_open = false;
        self.session_picker_open = false;
        self.finish_current_model_item();
        self.reproject_body();
        self.finish_active_goal_without_runtime_event("waiting");
    }

    /// Mark a run finished with durable ids.
    pub fn finish_run(&mut self, session_id: Option<String>) {
        self.stop_session_timer();
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
        self.tool_items_by_call_id.clear();
        self.tool_items_by_key.clear();
        self.tool_call_arguments.clear();
        self.subagent_states.clear();
        self.pending_hitl = None;
        self.hitl_reload_session_id = None;
        self.pending_submission_display_prompt = None;
        self.model_picker_open = false;
        self.session_picker_open = false;
        self.finish_current_model_item();
        self.reproject_body();
        self.finish_active_goal_without_runtime_event("unverified_stop");
    }

    /// Mark a run failed.
    pub fn fail_run(&mut self, error: &str) {
        self.stop_session_timer();
        self.running = false;
        self.cancel_requested = false;
        self.status = "ERROR".to_string();
        self.phase = "failed".to_string();
        self.streaming_parts.clear();
        self.streaming_tool_calls.clear();
        self.visible_tool_calls.clear();
        self.tool_items_by_call_id.clear();
        self.tool_items_by_key.clear();
        self.tool_call_arguments.clear();
        self.subagent_states.clear();
        self.pending_hitl = None;
        self.hitl_reload_session_id = None;
        self.pending_submission_display_prompt = None;
        self.model_picker_open = false;
        self.session_picker_open = false;
        self.finish_active_goal_without_runtime_event("error");
        self.push_system_notice(NoticeLevel::Error, format!("Error: {error}"));
    }

    /// Mark a run cancelled by the interactive user.
    pub fn cancel_run(&mut self, reason: &str) {
        self.stop_session_timer();
        self.running = false;
        self.cancel_requested = false;
        self.status = "IDLE".to_string();
        self.phase = "cancelled".to_string();
        self.streaming_parts.clear();
        self.streaming_tool_calls.clear();
        self.visible_tool_calls.clear();
        self.tool_items_by_call_id.clear();
        self.tool_items_by_key.clear();
        self.tool_call_arguments.clear();
        self.subagent_states.clear();
        self.pending_hitl = None;
        self.hitl_reload_session_id = None;
        self.pending_submission_display_prompt = None;
        self.model_picker_open = false;
        self.session_picker_open = false;
        self.finish_active_goal_without_runtime_event("cancelled");
        self.push_run_status_line(format!("Run cancelled: {reason}"));
    }

    pub(super) fn take_pending_clear_context(&mut self) -> bool {
        std::mem::take(&mut self.pending_clear_context)
    }

    pub(crate) fn reject_context_clear(&mut self, status: impl Into<String>) {
        self.input_status = Some(status.into());
    }

    pub(crate) fn clear_context_view(&mut self) {
        self.session_id = None;
        self.clear_composer();
        self.clear_persisted_history();
        self.timeline.clear();
        self.timeline_projection = TuiProjection::default();
        self.body.clear();
        self.rendered_body_len = 0;
        self.active_model_segment = None;
        self.reset_session_metrics();
        self.model_transport_status = None;
        self.streaming_parts.clear();
        self.streaming_text_seen = false;
        self.streaming_reasoning_seen = false;
        self.visible_text_seen = false;
        self.streaming_tool_calls.clear();
        self.visible_tool_calls.clear();
        self.tool_items_by_call_id.clear();
        self.tool_items_by_key.clear();
        self.tool_call_arguments.clear();
        self.subagent_states.clear();
        self.pending_hitl = None;
        self.hitl_reload_session_id = None;
        self.reset_task_panel_for_session();
        self.pending_clear_context = false;
        self.selection_mode = false;
        self.selection_index = None;
        self.pending_submission_display_prompt = None;
        self.model_picker_open = false;
        self.session_picker_open = false;
        self.pending_session_command = None;
        self.pending_shell_command = None;
        self.running = false;
        self.shell_running = false;
        self.cancel_requested = false;
        self.goal_task = None;
        self.goal_active = false;
        self.goal_iteration = 0;
        self.pending_goal_submission = None;
        self.footer_mode = FooterMode::Context;
        self.input_status = Some("context cleared".to_string());
        self.phase = "cleared".to_string();
        self.status = "IDLE".to_string();
        self.scroll_to_bottom();
    }

    #[cfg(test)]
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
            "READY"
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

    pub(super) fn input_notification(&self) -> Option<&str> {
        self.input_status.as_deref()
    }

    pub(super) const fn help_panel_visible(&self) -> bool {
        self.help_panel_open
    }

    pub(super) fn open_help_panel(&mut self) {
        self.help_panel_open = true;
        self.input_status = Some("help".to_string());
    }

    pub(super) fn close_help_panel(&mut self) {
        self.help_panel_open = false;
        self.input_status = Some("help closed".to_string());
    }

    /// Set config-defined slash commands shown by `/help` and expanded before submit.
    pub fn set_custom_commands(&mut self, commands: BTreeMap<String, SlashCommandDefinition>) {
        self.custom_commands = commands;
        self.footer_mode = FooterMode::Context;
        self.refresh_command_palette();
    }

    pub(super) const fn pending_hitl(&self) -> Option<&HitlPanelState> {
        self.pending_hitl.as_ref()
    }

    pub(super) fn hitl_decision_ready(&self) -> bool {
        self.pending_hitl
            .as_ref()
            .is_some_and(|hitl| hitl.approval_id.is_some())
    }

    pub(super) fn clarifying_answer_ready(&self) -> bool {
        self.pending_hitl
            .as_ref()
            .is_some_and(|hitl| hitl.approval_id.is_some() && hitl.clarifying.is_some())
    }

    pub(super) fn clarifying_question(&self) -> Option<&ClarifyingQuestionUiState> {
        self.pending_hitl
            .as_ref()
            .and_then(|hitl| hitl.clarifying.as_ref())
    }

    pub(super) fn clarifying_free_form_active(&self) -> bool {
        self.clarifying_question()
            .is_some_and(|question| question.free_form_active)
    }

    pub(super) fn move_clarifying_option(&mut self, delta: isize) {
        let Some(clarifying) = self
            .pending_hitl
            .as_mut()
            .and_then(|hitl| hitl.clarifying.as_mut())
        else {
            return;
        };
        let Some(question) = clarifying.questions.get(clarifying.question_index) else {
            return;
        };
        let len = question.options.len();
        if len == 0 {
            return;
        }
        clarifying.free_form_active = false;
        let steps = delta.unsigned_abs() % len;
        clarifying.option_index = if delta.is_negative() {
            (clarifying.option_index + len - steps) % len
        } else {
            (clarifying.option_index + steps) % len
        };
        self.input_status = Some("question choice".to_string());
    }

    pub(super) fn move_clarifying_question(&mut self, delta: isize) {
        let Some(clarifying) = self
            .pending_hitl
            .as_mut()
            .and_then(|hitl| hitl.clarifying.as_mut())
        else {
            return;
        };
        let len = clarifying.questions.len();
        if len == 0 {
            return;
        }
        let steps = delta.unsigned_abs() % len;
        clarifying.question_index = if delta.is_negative() {
            (clarifying.question_index + len - steps) % len
        } else {
            (clarifying.question_index + steps) % len
        };
        clarifying.option_index = 0;
        clarifying.free_form_active = false;
        self.clear_composer_input();
        self.input_status = Some("question changed".to_string());
    }

    pub(super) fn toggle_clarifying_selection(&mut self) {
        let Some(clarifying) = self
            .pending_hitl
            .as_mut()
            .and_then(|hitl| hitl.clarifying.as_mut())
        else {
            return;
        };
        let Some(question) = clarifying.questions.get(clarifying.question_index) else {
            return;
        };
        let Some(selection) = clarifying.selections.get_mut(clarifying.question_index) else {
            return;
        };
        clarifying
            .free_form_answers
            .remove(&clarifying.question_index);
        clarifying.free_form_active = false;
        if question.multi_select {
            if !selection.insert(clarifying.option_index) {
                selection.remove(&clarifying.option_index);
            }
        } else {
            selection.clear();
            selection.insert(clarifying.option_index);
        }
        self.input_status = Some("question selection updated".to_string());
    }

    pub(super) fn enter_clarifying_free_form(&mut self) {
        let Some(clarifying) = self
            .pending_hitl
            .as_mut()
            .and_then(|hitl| hitl.clarifying.as_mut())
        else {
            return;
        };
        clarifying.free_form_active = true;
        if let Some(answer) = clarifying
            .free_form_answers
            .get(&clarifying.question_index)
            .cloned()
        {
            self.input = answer;
            self.move_composer_cursor_to_end();
        } else {
            self.clear_composer_input();
        }
        self.input_status = Some("type a custom answer; Enter confirms".to_string());
    }

    pub(super) fn leave_clarifying_free_form(&mut self) {
        if let Some(clarifying) = self
            .pending_hitl
            .as_mut()
            .and_then(|hitl| hitl.clarifying.as_mut())
        {
            clarifying.free_form_active = false;
        }
        self.clear_composer_input();
        self.input_status = Some("question choices".to_string());
    }

    pub(super) fn confirm_clarifying_answer(&mut self) -> Option<ClarifyingQuestionAnswers> {
        let draft = self.input.trim().to_string();
        let (answers, next_label) = {
            let clarifying = self
                .pending_hitl
                .as_mut()
                .and_then(|hitl| hitl.clarifying.as_mut())?;
            let index = clarifying.question_index;
            if clarifying.free_form_active {
                if draft.is_empty() {
                    self.input_status = Some("custom answer cannot be empty".to_string());
                    return None;
                }
                clarifying.free_form_answers.insert(index, draft);
                clarifying.selections[index].clear();
            } else {
                let question = clarifying.questions.get(index)?;
                let selection = clarifying.selections.get_mut(index)?;
                if question.multi_select {
                    if selection.is_empty() {
                        self.input_status = Some("select at least one option".to_string());
                        return None;
                    }
                } else {
                    selection.clear();
                    selection.insert(clarifying.option_index);
                }
                clarifying.free_form_answers.remove(&index);
            }
            clarifying.free_form_active = false;
            if index + 1 < clarifying.questions.len() {
                clarifying.question_index += 1;
                clarifying.option_index = 0;
                (
                    None,
                    Some(format!(
                        "question {}/{}",
                        clarifying.question_index + 1,
                        clarifying.questions.len()
                    )),
                )
            } else {
                (clarifying.answers(), None)
            }
        };
        self.clear_composer_input();
        if let Some(label) = next_label {
            self.input_status = Some(label);
            return None;
        }
        if answers.is_none() {
            self.input_status = Some("answer every question before submitting".to_string());
        }
        answers
    }

    pub(crate) fn commit_clarifying_answer(&mut self) {
        self.clear_composer_input();
        self.reset_composer_scroll();
        self.input_status = Some("answers submitted".to_string());
    }

    pub(crate) fn hitl_reload_session_id(&self) -> Option<&str> {
        self.hitl_reload_session_id.as_deref()
    }

    pub(crate) fn require_hitl_reload(&mut self, session_id: impl Into<String>) {
        self.hitl_reload_session_id = Some(session_id.into());
        self.status = "WAITING".to_string();
        self.phase = "hitl refresh".to_string();
        self.input_status = Some("HITL: [Esc] refresh".to_string());
    }

    pub(crate) fn clear_hitl_reload_required(&mut self) {
        self.hitl_reload_session_id = None;
    }

    pub(crate) fn bind_pending_approval(&mut self, approval: &starweaver_session::ApprovalRecord) {
        let existing = self.pending_hitl.take();
        self.hitl_reload_session_id = Some(approval.session_id.as_str().to_string());
        let request_string = |key: &str| {
            approval
                .request
                .get(key)
                .and_then(Value::as_str)
                .map(ToString::to_string)
        };
        let clarifying = if approval.action_name == starweaver_agent::ASK_USER_QUESTION_TOOL_NAME {
            ClarifyingQuestionUiState::from_request(&approval.request)
                .or_else(|| existing.as_ref().and_then(|hitl| hitl.clarifying.clone()))
        } else {
            None
        };
        self.pending_hitl = Some(HitlPanelState {
            approval_id: Some(approval.approval_id.clone()),
            tool_call_id: approval.action_id.clone(),
            tool_name: approval.action_name.clone(),
            request_preview: Some(approval_request_preview(&approval.request)),
            clarifying,
            command: request_string("command")
                .or_else(|| request_string("script"))
                .or_else(|| existing.as_ref().and_then(|hitl| hitl.command.clone())),
            risk_level: request_string("risk_level")
                .or_else(|| existing.as_ref().and_then(|hitl| hitl.risk_level.clone())),
            reason: request_string("reason")
                .or_else(|| existing.as_ref().and_then(|hitl| hitl.reason.clone())),
        });
        self.status = "WAITING".to_string();
        if self.clarifying_answer_ready() {
            self.phase = "clarifying question".to_string();
            self.input_status = Some("use arrows and Enter; E for custom answer".to_string());
        } else {
            self.phase = "hitl approval".to_string();
            self.input_status = Some("approval: [a/y] approve, [r/n] reject".to_string());
        }
    }

    pub(crate) fn clear_pending_hitl(&mut self) {
        self.pending_hitl = None;
    }

    pub(super) fn task_panel_items(&self) -> &[TaskPanelItem] {
        &self.task_panel_items
    }

    fn record_steering_message(&mut self, text: String) -> SteeringSubmission {
        let id = format!("steer_{}", self.next_steering_id);
        self.next_steering_id = self.next_steering_id.saturating_add(1);
        self.push_system_notice(NoticeLevel::Info, format!("Steering: {text}"));
        SteeringSubmission { id, text }
    }

    pub(super) const fn scroll_to_bottom(&mut self) {
        self.scroll_offset = usize::MAX;
        self.unread_output_lines = 0;
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

    pub(super) const fn activity_running(&self) -> bool {
        self.running || self.shell_running
    }

    pub(super) const fn shell_running(&self) -> bool {
        self.shell_running
    }

    pub(super) fn request_cancel(&mut self) {
        let already_requested = self.cancel_requested;
        self.cancel_requested = true;
        self.status = "INTERRUPT".to_string();
        self.phase = "cancel requested".to_string();
        if !already_requested {
            self.push_system_notice(
                NoticeLevel::Warning,
                "Interrupt requested. Cancelling active activity.".to_string(),
            );
        }
    }

    pub(super) fn show_run_active_hint(&mut self) {
        self.status = "RUNNING".to_string();
        self.phase = "run active; press Ctrl-C to interrupt".to_string();
    }

    pub(super) fn show_draft_exit_hint(&mut self) {
        self.input_status = Some("draft preserved; Ctrl-U clears it".to_string());
    }

    pub(super) fn show_shell_active_hint(&mut self) {
        self.input_status = Some("shell active; draft preserved; Ctrl-C cancels it".to_string());
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
        self.push_system_notice(
            NoticeLevel::Info,
            format!(
                "[Goal] Total tokens: {} (input: {}, cache read: {}, cache write: {}, output: {})",
                format_u64_with_commas(usage.total_tokens),
                format_u64_with_commas(usage.input_tokens),
                format_u64_with_commas(usage.cache_read_tokens),
                format_u64_with_commas(usage.cache_write_tokens),
                format_u64_with_commas(usage.output_tokens)
            ),
        );
    }

    fn finish_active_goal_without_runtime_event(&mut self, reason: &str) {
        if self.goal_active {
            self.goal_active = false;
            self.push_system_notice(NoticeLevel::Info, format!("[Goal] Completed: {reason}"));
            self.push_goal_total_tokens_report();
        }
        self.pending_goal_submission = None;
    }
}

pub(super) fn approval_request_preview(request: &Value) -> String {
    const MAX_PREVIEW_CHARS: usize = 2_000;

    let serialized = serde_json::to_string(request).unwrap_or_else(|_| request.to_string());
    let mut chars = serialized.chars();
    let mut preview = chars.by_ref().take(MAX_PREVIEW_CHARS).collect::<String>();
    if chars.next().is_some() {
        preview.push('…');
    }
    preview
}

fn format_session_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    let hours = seconds / 3_600;
    let minutes = (seconds % 3_600) / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        format!("{hours}h{minutes:02}m{seconds:02}s")
    } else if minutes > 0 {
        format!("{minutes}m{seconds:02}s")
    } else {
        format!("{seconds}s")
    }
}

const fn render_mode_label(render_mode: TuiRenderMode) -> &'static str {
    match render_mode {
        TuiRenderMode::Normal => "normal",
        TuiRenderMode::Concise => "concise",
        TuiRenderMode::Debug => "debug",
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
