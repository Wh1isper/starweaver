use std::{
    io::{self, Write},
    time::Duration,
};

use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent, MouseEventKind,
    },
    execute, queue,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};

#[cfg(unix)]
use crossterm::event::{
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};

use crate::{CliResult, prompt_input::PromptInput};
use starweaver_agent::ClarifyingQuestionAnswers;

use super::{
    render::{
        StyledLine, composer_input_width, composer_layout, queue_styled_line_at,
        render_composer_lines_from_layout, render_footer_lines, render_live_history_lines,
        render_status_bar_lines, terminal_error,
    },
    state::{
        BodyScrollDirection, COMPOSER_VISIBLE_LINES, CommandPaletteAccept, InteractiveTuiState,
        PendingSessionCommand, SteeringSubmission,
    },
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TuiApprovalDecision {
    Approve,
    Reject,
    Answer(ClarifyingQuestionAnswers),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InteractiveTuiEvent {
    /// Redraw after a handled key changed or may have changed local UI state.
    Redraw,
    /// Submit a prompt.
    Submit(PromptInput),
    /// Send steering to the active run UI pane.
    Steer(SteeringSubmission),
    /// Reload or list sessions from the service-owned local store.
    Session(Option<String>),
    /// Clear visible transcript and detach the active session context.
    Clear,
    /// Attach an image from the system clipboard.
    PasteImage,
    /// Start a local shell activity without blocking terminal input.
    Shell(String),
    /// Resolve the durable approval currently shown by the HITL panel.
    ApprovalDecision(TuiApprovalDecision),
    /// Interrupt the active activity.
    Cancel,
    /// Quit the TUI.
    Quit,
}

/// Interactive terminal UI session.
pub struct InteractiveTui {
    stdout: io::Stdout,
    active: bool,
    mouse_capture_enabled: bool,
    keyboard_enhancements_enabled: bool,
    rendered_body_cache: RenderedBodyCache,
    frame_cache: FrameCache,
}

#[derive(Debug, Default)]
struct RenderedBodyCache {
    signature: Option<BodyRenderSignature>,
    lines: Vec<StyledLine>,
}

#[derive(Debug, Default)]
struct FrameCache {
    width: usize,
    height: usize,
    lines: Vec<StyledLine>,
}

impl FrameCache {
    fn reset_if_geometry_changed(&mut self, width: usize, height: usize) {
        if self.width == width && self.height == height {
            return;
        }
        self.width = width;
        self.height = height;
        self.lines.clear();
    }

    fn line_changed(&self, row: usize, line: &StyledLine) -> bool {
        self.lines.get(row) != Some(line)
    }

    fn set_line(&mut self, row: usize, line: StyledLine) {
        if self.lines.len() <= row {
            self.lines
                .resize_with(row.saturating_add(1), || StyledLine::plain(""));
        }
        self.lines[row] = line;
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct BodyRenderSignature {
    width: usize,
    workspace_dir: String,
    model: String,
    render_mode: crate::args::TuiRenderMode,
    timeline_generation: u64,
    body_len: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ResponsiveFrameBudget {
    pub(super) body: usize,
    pub(super) panels: usize,
    pub(super) status: usize,
    pub(super) composer: usize,
    pub(super) bottom_padding: usize,
}

pub(super) fn responsive_frame_budget(
    height: usize,
    desired_composer: usize,
    desired_status: usize,
    available_panel_lines: usize,
) -> ResponsiveFrameBudget {
    let height = height.max(1);
    let has_panel = available_panel_lines > 0;
    let mut composer = 1usize;
    let mut status = usize::from(height >= 2);
    let mut body = usize::from(!has_panel && height >= 3);
    let bottom_padding = usize::from((!has_panel && height >= 6) || (has_panel && height >= 12));
    let mut remaining = height.saturating_sub(composer + status + body + bottom_padding);

    // Active panels own the footer interaction. Reserve their rows before
    // passive status metadata so compact questions, approvals, tasks, and
    // pickers retain visible content as well as an action.
    let panels = if has_panel {
        available_panel_lines.min(remaining)
    } else {
        0
    };
    remaining = remaining.saturating_sub(panels);

    let status_extra = desired_status
        .max(status)
        .saturating_sub(status)
        .min(remaining);
    status = status.saturating_add(status_extra);
    remaining = remaining.saturating_sub(status_extra);

    let composer_extra = desired_composer.max(1).saturating_sub(1).min(remaining);
    composer = composer.saturating_add(composer_extra);
    remaining = remaining.saturating_sub(composer_extra);

    body = body.saturating_add(remaining);

    ResponsiveFrameBudget {
        body,
        panels,
        status,
        composer,
        bottom_padding,
    }
}

fn compact_panel_lines(
    panel_lines: &[StyledLine],
    budget: usize,
    _width: usize,
) -> Vec<StyledLine> {
    if budget == 0 {
        return Vec::new();
    }
    if panel_lines.len() <= budget {
        return panel_lines.to_vec();
    }
    let meaningful = panel_lines
        .iter()
        .filter_map(|line| panel_line_is_meaningful(line).then_some(line.clone()))
        .collect::<Vec<_>>();
    if meaningful.len() <= budget {
        return meaningful;
    }
    if budget == 1 {
        return meaningful.last().cloned().into_iter().collect();
    }
    let mut lines = meaningful
        .iter()
        .take(budget.saturating_sub(1))
        .cloned()
        .collect::<Vec<_>>();
    if let Some(action) = meaningful.last()
        && lines.last() != Some(action)
    {
        lines.push(action.clone());
    }
    lines.truncate(budget);
    lines
}

fn panel_line_is_meaningful(line: &StyledLine) -> bool {
    let text = line
        .segments
        .iter()
        .map(|segment| segment.text.as_str())
        .collect::<String>();
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.starts_with('╭') || trimmed.starts_with('╰') {
        return false;
    }
    !trimmed.trim_matches('│').trim().is_empty()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct RenderedFrame {
    pub(super) lines: Vec<StyledLine>,
    pub(super) cursor_row: usize,
    pub(super) cursor_col: usize,
    pub(super) render_width: usize,
}

#[cfg(test)]
pub(super) fn compose_frame(
    state: &mut InteractiveTuiState,
    terminal_width: usize,
    height: usize,
) -> RenderedFrame {
    let render_width = terminal_width.saturating_sub(1).max(1);
    let rendered_body = render_live_history_lines(state, render_width);
    compose_frame_from_body(state, render_width, height, &rendered_body)
}

fn compose_frame_from_body(
    state: &mut InteractiveTuiState,
    render_width: usize,
    height: usize,
    rendered_body: &[StyledLine],
) -> RenderedFrame {
    let height = height.max(1);
    let input_width = composer_input_width(render_width);
    state.update_composer_content_width(input_width);
    let preview_layout = composer_layout(
        &state.input,
        state.composer_cursor_byte(),
        COMPOSER_VISIBLE_LINES,
        state.composer_scroll_offset(),
        input_width,
    );
    let all_footer_lines = render_footer_lines(state, render_width);
    let desired_status = render_status_bar_lines(state, render_width).len();
    let status_index = all_footer_lines.len().saturating_sub(desired_status);
    let (panel_lines, all_status_lines) = all_footer_lines.split_at(status_index);
    let desired_composer = preview_layout.visible_lines.len().saturating_add(1);
    let budget =
        responsive_frame_budget(height, desired_composer, desired_status, panel_lines.len());
    let composer_visible_lines = if budget.composer > 1 {
        budget.composer.saturating_sub(1)
    } else {
        1
    };
    let composer_layout = composer_layout(
        &state.input,
        state.composer_cursor_byte(),
        composer_visible_lines,
        state.composer_scroll_offset(),
        input_width,
    );
    let mut composer_lines =
        render_composer_lines_from_layout(state, render_width, &composer_layout);
    let composer_has_spacer = budget.composer > 1;
    if !composer_has_spacer && !composer_lines.is_empty() {
        composer_lines.remove(0);
    }
    composer_lines.truncate(budget.composer);
    let status_lines = if budget.status == 0 {
        Vec::new()
    } else {
        all_status_lines
            .iter()
            .take(budget.status)
            .cloned()
            .collect::<Vec<_>>()
    };
    let panel_lines = compact_panel_lines(panel_lines, budget.panels, render_width);
    let rendered_body_len = rendered_body.len();
    state.update_render_metrics(rendered_body_len, budget.body);
    let (visible_start, visible_end) = visible_body_bounds(state, rendered_body_len, budget.body);
    let visible_body = &rendered_body[visible_start..visible_end];
    let mut lines = vec![StyledLine::plain(""); height];
    for (row, line) in visible_body.iter().take(budget.body).enumerate() {
        lines[row] = line.clone();
    }
    let panel_start = budget.body;
    for (offset, line) in panel_lines.iter().enumerate() {
        if let Some(slot) = lines.get_mut(panel_start.saturating_add(offset)) {
            *slot = line.clone();
        }
    }
    let status_start = panel_start.saturating_add(panel_lines.len());
    for (offset, line) in status_lines.iter().enumerate() {
        if let Some(slot) = lines.get_mut(status_start.saturating_add(offset)) {
            *slot = line.clone();
        }
    }
    let composer_start = status_start.saturating_add(status_lines.len());
    for (offset, line) in composer_lines.iter().enumerate() {
        if let Some(slot) = lines.get_mut(composer_start.saturating_add(offset)) {
            *slot = line.clone();
        }
    }
    let cursor_row = composer_start
        .saturating_add(usize::from(composer_has_spacer))
        .saturating_add(
            composer_layout
                .cursor_line
                .saturating_sub(composer_layout.visible_start)
                .min(composer_layout.visible_lines.len().saturating_sub(1)),
        )
        .min(height.saturating_sub(1));
    let cursor_col = 2usize
        .saturating_add(composer_layout.cursor_col)
        .min(render_width.saturating_sub(1));
    RenderedFrame {
        lines,
        cursor_row,
        cursor_col,
        render_width,
    }
}

impl InteractiveTui {
    /// Enter Codex-style inline interactive mode.
    pub fn enter() -> CliResult<Self> {
        let mut stdout = io::stdout();
        terminal::enable_raw_mode().map_err(terminal_error)?;
        if let Err(error) = execute!(
            stdout,
            EnterAlternateScreen,
            EnableBracketedPaste,
            EnableMouseCapture,
            Hide
        ) {
            let _ = execute!(
                stdout,
                Show,
                DisableMouseCapture,
                DisableBracketedPaste,
                LeaveAlternateScreen
            );
            let _ = terminal::disable_raw_mode();
            return Err(terminal_error(error));
        }
        let keyboard_enhancements_enabled = enable_keyboard_enhancements(&mut stdout);
        Ok(Self {
            stdout,
            active: true,
            mouse_capture_enabled: true,
            keyboard_enhancements_enabled,
            rendered_body_cache: RenderedBodyCache::default(),
            frame_cache: FrameCache::default(),
        })
    }

    /// Restore terminal modes immediately. Calling this more than once is safe.
    pub fn restore(&mut self) -> CliResult<()> {
        if !self.active {
            return Ok(());
        }
        let mut first_error = None;
        if self.keyboard_enhancements_enabled {
            #[cfg(unix)]
            if let Err(error) = execute!(self.stdout, PopKeyboardEnhancementFlags) {
                first_error.get_or_insert_with(|| terminal_error(error));
            }
            self.keyboard_enhancements_enabled = false;
        }
        if let Err(error) = execute!(
            self.stdout,
            Show,
            DisableMouseCapture,
            DisableBracketedPaste,
            LeaveAlternateScreen
        ) {
            first_error.get_or_insert_with(|| terminal_error(error));
        }
        if let Err(error) = terminal::disable_raw_mode() {
            first_error.get_or_insert_with(|| terminal_error(error));
        }
        self.mouse_capture_enabled = false;
        self.active = false;
        first_error.map_or(Ok(()), Err)
    }

    /// Render the current state.
    #[allow(clippy::too_many_lines)]
    pub fn render(&mut self, state: &mut InteractiveTuiState) -> CliResult<()> {
        self.sync_mouse_capture(should_capture_mouse(state))?;
        let (width, height) = terminal::size().unwrap_or((80, 24));
        let terminal_width = usize::from(width).max(1);
        let render_width = terminal_width.saturating_sub(1).max(1);
        let height = usize::from(height).max(1);
        let rendered_body = self.rendered_body_lines(state, render_width).to_vec();
        let frame = compose_frame_from_body(state, render_width, height, &rendered_body);
        self.frame_cache
            .reset_if_geometry_changed(frame.render_width, height);
        let changed_rows = frame
            .lines
            .iter()
            .enumerate()
            .filter(|(row, line)| self.frame_cache.line_changed(*row, line))
            .collect::<Vec<_>>();
        if !changed_rows.is_empty() {
            queue!(self.stdout, Hide).map_err(terminal_error)?;
            for (row, line) in changed_rows {
                queue_styled_line_at(
                    &mut self.stdout,
                    u16::try_from(row).unwrap_or(u16::MAX),
                    line,
                    frame.render_width,
                )?;
                self.frame_cache.set_line(row, line.clone());
            }
        }
        queue!(
            self.stdout,
            MoveTo(
                u16::try_from(frame.cursor_col).unwrap_or(u16::MAX),
                u16::try_from(frame.cursor_row).unwrap_or(u16::MAX),
            ),
            Show
        )
        .map_err(terminal_error)?;
        self.stdout.flush().map_err(terminal_error)
    }

    fn rendered_body_lines(&mut self, state: &InteractiveTuiState, width: usize) -> &[StyledLine] {
        let signature = body_render_signature(state, width);
        if self.rendered_body_cache.signature.as_ref() != Some(&signature) {
            self.rendered_body_cache.lines = render_live_history_lines(state, width);
            self.rendered_body_cache.signature = Some(signature);
        }
        &self.rendered_body_cache.lines
    }

    fn sync_mouse_capture(&mut self, should_enable: bool) -> CliResult<()> {
        if self.mouse_capture_enabled == should_enable {
            return Ok(());
        }
        if should_enable {
            execute!(self.stdout, EnableMouseCapture).map_err(terminal_error)?;
        } else {
            execute!(self.stdout, DisableMouseCapture).map_err(terminal_error)?;
        }
        self.mouse_capture_enabled = should_enable;
        Ok(())
    }

    /// Poll for one UI event while keeping the caller-owned event loop responsive.
    pub fn poll_event(
        state: &mut InteractiveTuiState,
        timeout: Duration,
    ) -> CliResult<Option<InteractiveTuiEvent>> {
        if !event::poll(timeout).map_err(terminal_error)? {
            return Ok(None);
        }
        match event::read().map_err(terminal_error)? {
            Event::Key(key)
                if key.kind == KeyEventKind::Press || key.kind == KeyEventKind::Repeat =>
            {
                Ok(handle_key_event(state, key).or(Some(InteractiveTuiEvent::Redraw)))
            }
            Event::Paste(text) => {
                state.apply_paste(&text);
                Ok(Some(InteractiveTuiEvent::Redraw))
            }
            Event::Mouse(mouse) => Ok(handle_mouse_event(state, mouse)),
            Event::Resize(_, _) => Ok(Some(InteractiveTuiEvent::Redraw)),
            _ => Ok(None),
        }
    }
}

fn body_render_signature(state: &InteractiveTuiState, width: usize) -> BodyRenderSignature {
    BodyRenderSignature {
        width,
        workspace_dir: state.workspace_dir.clone(),
        model: state.model.clone(),
        render_mode: state.render_mode(),
        timeline_generation: state.timeline_generation(),
        body_len: state.body.len(),
    }
}

fn scroll_viewport(
    state: &mut InteractiveTuiState,
    amount: usize,
    direction: BodyScrollDirection,
) -> bool {
    state.scroll_body(amount, direction)
}

pub(super) fn handle_mouse_event(
    state: &mut InteractiveTuiState,
    mouse: MouseEvent,
) -> Option<InteractiveTuiEvent> {
    match mouse.kind {
        MouseEventKind::ScrollUp => scroll_viewport(state, 3, BodyScrollDirection::Up)
            .then_some(InteractiveTuiEvent::Redraw),
        MouseEventKind::ScrollDown => scroll_viewport(state, 3, BodyScrollDirection::Down)
            .then_some(InteractiveTuiEvent::Redraw),
        _ => None,
    }
}

fn session_command_event(command: PendingSessionCommand) -> InteractiveTuiEvent {
    match command {
        PendingSessionCommand::Current => InteractiveTuiEvent::Session(None),
        PendingSessionCommand::Select(session_id) => InteractiveTuiEvent::Session(Some(session_id)),
    }
}

pub(super) const fn should_capture_mouse(state: &InteractiveTuiState) -> bool {
    !state.selection_mode_visible()
}

pub(super) fn visible_body_bounds(
    state: &InteractiveTuiState,
    rendered_body_len: usize,
    body_height: usize,
) -> (usize, usize) {
    let max_scroll = rendered_body_len.saturating_sub(body_height);
    let visible_start = if state.is_at_bottom() {
        max_scroll
    } else {
        state.scroll_offset.min(max_scroll)
    };
    let visible_end = visible_start
        .saturating_add(body_height)
        .min(rendered_body_len);
    (visible_start, visible_end)
}

fn command_modifier(modifiers: KeyModifiers) -> bool {
    modifiers.intersects(KeyModifiers::SUPER | KeyModifiers::META)
}

fn word_modifier(modifiers: KeyModifiers) -> bool {
    modifiers.intersects(KeyModifiers::ALT | KeyModifiers::CONTROL)
}

#[cfg(unix)]
fn enable_keyboard_enhancements(stdout: &mut io::Stdout) -> bool {
    execute!(
        stdout,
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    )
    .is_ok()
}

#[cfg(not(unix))]
fn enable_keyboard_enhancements(stdout: &mut io::Stdout) -> bool {
    let _ = stdout;
    false
}

#[allow(clippy::too_many_lines)]
pub(super) fn handle_key_event(
    state: &mut InteractiveTuiState,
    key: KeyEvent,
) -> Option<InteractiveTuiEvent> {
    if key.code == KeyCode::Char('c')
        && key.modifiers.contains(KeyModifiers::CONTROL)
        && state.activity_running()
    {
        state.request_cancel();
        return Some(InteractiveTuiEvent::Cancel);
    }
    if state.clarifying_answer_ready() {
        if state.clarifying_free_form_active() {
            match key.code {
                KeyCode::Enter => {
                    return state.confirm_clarifying_answer().map(|answers| {
                        InteractiveTuiEvent::ApprovalDecision(TuiApprovalDecision::Answer(answers))
                    });
                }
                KeyCode::Esc => state.leave_clarifying_free_form(),
                KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    state.insert_composer_newline();
                }
                KeyCode::PageUp => {
                    scroll_viewport(state, 10, BodyScrollDirection::Up);
                }
                KeyCode::PageDown => {
                    scroll_viewport(state, 10, BodyScrollDirection::Down);
                }
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Some(InteractiveTuiEvent::Quit);
                }
                KeyCode::Backspace => state.backspace_composer(),
                KeyCode::Left => state.move_composer_cursor_left(),
                KeyCode::Right => state.move_composer_cursor_right(),
                KeyCode::Home => state.move_composer_cursor_to_line_start(),
                KeyCode::End => state.move_composer_cursor_to_line_end(),
                KeyCode::Up => state.move_composer_cursor_vertical(-1),
                KeyCode::Down => state.move_composer_cursor_vertical(1),
                KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    state.push_composer_char(ch);
                }
                _ => {}
            }
            return None;
        }
        match key.code {
            KeyCode::Enter => {
                return state.confirm_clarifying_answer().map(|answers| {
                    InteractiveTuiEvent::ApprovalDecision(TuiApprovalDecision::Answer(answers))
                });
            }
            KeyCode::Up => state.move_clarifying_option(-1),
            KeyCode::Down => state.move_clarifying_option(1),
            KeyCode::Char(' ') => state.toggle_clarifying_selection(),
            KeyCode::Char('e') if key.modifiers.is_empty() => {
                state.enter_clarifying_free_form();
            }
            KeyCode::Tab => state.move_clarifying_question(1),
            KeyCode::BackTab => state.move_clarifying_question(-1),
            KeyCode::Esc => {
                let session_id = state
                    .hitl_reload_session_id()
                    .map(ToString::to_string)
                    .or_else(|| state.session_id.clone());
                return session_id.map(|session_id| InteractiveTuiEvent::Session(Some(session_id)));
            }
            KeyCode::PageUp => {
                scroll_viewport(state, 10, BodyScrollDirection::Up);
            }
            KeyCode::PageDown => {
                scroll_viewport(state, 10, BodyScrollDirection::Down);
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return Some(InteractiveTuiEvent::Quit);
            }
            _ => {}
        }
        return None;
    }
    if (state.pending_hitl().is_some() || state.hitl_reload_session_id().is_some())
        && !state.running
        && !state.clarifying_answer_ready()
    {
        match key.code {
            KeyCode::Char('a' | 'y') if key.modifiers.is_empty() && state.hitl_decision_ready() => {
                return Some(InteractiveTuiEvent::ApprovalDecision(
                    TuiApprovalDecision::Approve,
                ));
            }
            KeyCode::Char('r' | 'n') if key.modifiers.is_empty() && state.hitl_decision_ready() => {
                return Some(InteractiveTuiEvent::ApprovalDecision(
                    TuiApprovalDecision::Reject,
                ));
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return Some(InteractiveTuiEvent::Quit);
            }
            KeyCode::Esc => {
                let session_id = state
                    .hitl_reload_session_id()
                    .map(ToString::to_string)
                    .or_else(|| state.session_id.clone());
                return session_id.map(|session_id| InteractiveTuiEvent::Session(Some(session_id)));
            }
            KeyCode::PageUp => {
                scroll_viewport(state, 10, BodyScrollDirection::Up);
            }
            KeyCode::PageDown => {
                scroll_viewport(state, 10, BodyScrollDirection::Down);
            }
            _ => {}
        }
        return None;
    }
    if state.history_search_visible() {
        match key.code {
            KeyCode::Esc => state.close_history_search(),
            KeyCode::Enter => state.accept_history_search(),
            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                state.repeat_history_search();
            }
            KeyCode::Up => state.move_history_search(-1),
            KeyCode::Down => state.move_history_search(1),
            KeyCode::Backspace => state.backspace_history_search(),
            KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                state.push_history_search_char(ch);
            }
            _ => {}
        }
        return None;
    }
    if state.command_palette_visible() {
        let continue_input_handling = match key.code {
            KeyCode::Esc => {
                state.close_command_palette();
                return None;
            }
            KeyCode::Up | KeyCode::BackTab => {
                state.move_command_palette_selection(-1);
                return None;
            }
            KeyCode::Down => {
                state.move_command_palette_selection(1);
                return None;
            }
            KeyCode::Tab => {
                state.accept_command_palette_selection(false);
                return None;
            }
            KeyCode::Enter => {
                state.accept_command_palette_selection(true) == Some(CommandPaletteAccept::Execute)
            }
            _ => true,
        };
        if !continue_input_handling {
            return None;
        }
    }
    if state.session_picker_visible() {
        match key.code {
            KeyCode::Esc => state.close_session_picker(),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                state.close_session_picker();
            }
            KeyCode::Enter => {
                state.select_session_picker_choice();
                return state
                    .take_pending_session_command()
                    .map(session_command_event);
            }
            KeyCode::Up if key.modifiers.contains(KeyModifiers::CONTROL) => {
                scroll_viewport(state, 1, BodyScrollDirection::Up);
            }
            KeyCode::Down if key.modifiers.contains(KeyModifiers::CONTROL) => {
                scroll_viewport(state, 1, BodyScrollDirection::Down);
            }
            KeyCode::PageUp => {
                scroll_viewport(state, 10, BodyScrollDirection::Up);
            }
            KeyCode::PageDown => {
                scroll_viewport(state, 10, BodyScrollDirection::Down);
            }
            KeyCode::Up => state.move_session_picker_selection(-1),
            KeyCode::Down => state.move_session_picker_selection(1),
            _ => {}
        }
        return None;
    }
    if state.model_picker_visible() {
        match key.code {
            KeyCode::Esc => state.close_model_picker(),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                state.close_model_picker();
            }
            KeyCode::Enter => state.select_model_picker_choice(),
            KeyCode::Up if key.modifiers.contains(KeyModifiers::CONTROL) => {
                scroll_viewport(state, 1, BodyScrollDirection::Up);
            }
            KeyCode::Down if key.modifiers.contains(KeyModifiers::CONTROL) => {
                scroll_viewport(state, 1, BodyScrollDirection::Down);
            }
            KeyCode::PageUp => {
                scroll_viewport(state, 10, BodyScrollDirection::Up);
            }
            KeyCode::PageDown => {
                scroll_viewport(state, 10, BodyScrollDirection::Down);
            }
            KeyCode::Up => state.move_model_picker_selection(-1),
            KeyCode::Down => state.move_model_picker_selection(1),
            _ => {}
        }
        return None;
    }
    if key.code == KeyCode::F(2) {
        state.toggle_task_panel();
        return None;
    }
    if state.help_panel_visible() {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::F(1) => state.close_help_panel(),
            KeyCode::PageUp => {
                scroll_viewport(state, 10, BodyScrollDirection::Up);
            }
            KeyCode::PageDown => {
                scroll_viewport(state, 10, BodyScrollDirection::Down);
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                state.close_help_panel();
            }
            _ => {}
        }
        return None;
    }
    if state.selection_mode_visible() {
        match key.code {
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => state.close_selection_mode(),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                state.close_selection_mode();
            }
            KeyCode::PageUp => {
                state.move_selection(-10);
                scroll_viewport(state, 10, BodyScrollDirection::Up);
            }
            KeyCode::PageDown => {
                state.move_selection(10);
                scroll_viewport(state, 10, BodyScrollDirection::Down);
            }
            KeyCode::Up if key.modifiers.contains(KeyModifiers::CONTROL) => {
                scroll_viewport(state, 1, BodyScrollDirection::Up);
            }
            KeyCode::Down if key.modifiers.contains(KeyModifiers::CONTROL) => {
                scroll_viewport(state, 1, BodyScrollDirection::Down);
            }
            KeyCode::Up => state.move_selection(-1),
            KeyCode::Down => state.move_selection(1),
            _ => {}
        }
        return None;
    }
    match key.code {
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if state.composer_is_empty() {
                return Some(InteractiveTuiEvent::Quit);
            }
            state.clear_composer();
        }
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if state.activity_running() {
                state.show_run_active_hint();
            } else if state.composer_is_empty() {
                return Some(InteractiveTuiEvent::Quit);
            } else {
                state.show_draft_exit_hint();
            }
        }
        KeyCode::Char('l') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.scroll_to_bottom();
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.clear_composer();
        }
        KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.move_composer_cursor_to_line_start();
        }
        KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.move_composer_cursor_to_line_end();
        }
        KeyCode::Char('b') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.move_composer_cursor_left();
        }
        KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.move_composer_cursor_right();
        }
        KeyCode::Char('b') if key.modifiers.contains(KeyModifiers::ALT) => {
            state.move_composer_cursor_word_left();
        }
        KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::ALT) => {
            state.move_composer_cursor_word_right();
        }
        KeyCode::Up if key.modifiers.contains(KeyModifiers::ALT) => {
            state.scroll_composer_up(1);
        }
        KeyCode::Down if key.modifiers.contains(KeyModifiers::ALT) => {
            state.scroll_composer_down(1);
        }
        KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            return Some(InteractiveTuiEvent::PasteImage);
        }
        KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.insert_composer_newline();
        }
        KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.previous_history();
        }
        KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.next_history();
        }
        KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.open_history_search();
        }
        KeyCode::F(1) => {
            state.open_help_panel();
        }
        KeyCode::Char('?') if key.modifiers.is_empty() && state.composer_is_empty() => {
            state.open_help_panel();
        }
        KeyCode::Esc if state.task_panel_expanded() => {}
        KeyCode::Esc => {
            state.open_selection_mode();
        }
        KeyCode::Tab => {
            state.refresh_command_palette();
        }
        KeyCode::Enter if state.shell_running() => {
            state.show_shell_active_hint();
        }
        KeyCode::Enter if state.running => {
            if state.take_paste_image_command() {
                return Some(InteractiveTuiEvent::PasteImage);
            }
            if let Some(steering) = state.take_steering_prompt() {
                state.push_history(steering.text.clone());
                return Some(InteractiveTuiEvent::Steer(steering));
            }
            if let Some(command) = state.take_pending_shell_command() {
                return Some(InteractiveTuiEvent::Shell(command));
            }
            if state.take_pending_clear_context() {
                return Some(InteractiveTuiEvent::Clear);
            }
        }
        KeyCode::Enter => {
            if state.take_paste_image_command() {
                return Some(InteractiveTuiEvent::PasteImage);
            }
            if let Some(prompt) = state.take_submission_prompt() {
                if let Some(history) = prompt.history_text() {
                    state.push_history(history);
                }
                return Some(InteractiveTuiEvent::Submit(prompt));
            }
            if let Some(command) = state.take_pending_shell_command() {
                return Some(InteractiveTuiEvent::Shell(command));
            }
            if state.take_pending_clear_context() {
                return Some(InteractiveTuiEvent::Clear);
            }
            if let Some(session) = state.take_pending_session_command() {
                return Some(session_command_event(session));
            }
        }
        KeyCode::Backspace => {
            state.backspace_composer();
        }
        KeyCode::Left if command_modifier(key.modifiers) => {
            state.move_composer_cursor_to_line_start();
        }
        KeyCode::Right if command_modifier(key.modifiers) => {
            state.move_composer_cursor_to_line_end();
        }
        KeyCode::Left if word_modifier(key.modifiers) => {
            state.move_composer_cursor_word_left();
        }
        KeyCode::Right if word_modifier(key.modifiers) => {
            state.move_composer_cursor_word_right();
        }
        KeyCode::Left => {
            state.move_composer_cursor_left();
        }
        KeyCode::Right => {
            state.move_composer_cursor_right();
        }
        KeyCode::Home => {
            state.move_composer_cursor_to_line_start();
        }
        KeyCode::End => {
            state.move_composer_cursor_to_line_end();
        }
        KeyCode::PageUp => {
            scroll_viewport(state, 10, BodyScrollDirection::Up);
        }
        KeyCode::PageDown => {
            scroll_viewport(state, 10, BodyScrollDirection::Down);
        }
        KeyCode::Up if key.modifiers.contains(KeyModifiers::CONTROL) => {
            scroll_viewport(state, 1, BodyScrollDirection::Up);
        }
        KeyCode::Down if key.modifiers.contains(KeyModifiers::CONTROL) => {
            scroll_viewport(state, 1, BodyScrollDirection::Down);
        }
        KeyCode::Up
            if !state.input.contains('\n')
                && (state.composer_is_empty() || state.history_recall_active()) =>
        {
            state.previous_history();
        }
        KeyCode::Down if !state.input.contains('\n') && state.history_recall_active() => {
            state.next_history();
        }
        KeyCode::Up => state.move_composer_cursor_vertical(-1),
        KeyCode::Down => state.move_composer_cursor_vertical(1),
        KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.push_composer_char(ch);
        }
        _ => {}
    }
    None
}

impl Drop for InteractiveTui {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}
