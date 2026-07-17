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

use super::{
    render::{
        StyledLine, composer_input_width, composer_layout, queue_styled_line_at,
        render_composer_lines_from_layout, render_footer_lines, render_live_history_lines,
        terminal_error,
    },
    state::{
        BodyScrollDirection, COMPOSER_VISIBLE_LINES, InteractiveTuiState, PendingSessionCommand,
        SteeringSubmission,
    },
};

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
    /// Interrupt the active run.
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
}

pub(super) fn responsive_frame_budget(
    height: usize,
    desired_composer: usize,
    available_panel_lines: usize,
) -> ResponsiveFrameBudget {
    let height = height.max(1);
    let mut composer = 1usize;
    let status = match height {
        1 => 0,
        2 | 3 => 1,
        _ => 2,
    };
    let mut body = usize::from(height >= 3);
    let mut remaining = height.saturating_sub(composer + status + body);

    let composer_extra = desired_composer.max(1).saturating_sub(1).min(remaining);
    composer = composer.saturating_add(composer_extra);
    remaining = remaining.saturating_sub(composer_extra);

    // Panels are useful, but they may not starve the transcript. Compact them
    // to at most half of the remaining rows and give all other rows to output.
    let panels = available_panel_lines.min(remaining / 2);
    body = body.saturating_add(remaining.saturating_sub(panels));

    ResponsiveFrameBudget {
        body,
        panels,
        status,
        composer,
    }
}

fn compact_panel_lines(panel_lines: &[StyledLine], budget: usize, width: usize) -> Vec<StyledLine> {
    if panel_lines.len() <= budget {
        return panel_lines.to_vec();
    }
    if budget == 0 {
        return Vec::new();
    }
    let hidden = panel_lines.len().saturating_sub(budget.saturating_sub(1));
    let mut lines = panel_lines
        .iter()
        .take(budget.saturating_sub(1))
        .cloned()
        .collect::<Vec<_>>();
    let notice = format!("… {hidden} panel line(s) hidden; enlarge terminal …");
    lines.push(StyledLine::plain(super::render::truncate_line(
        &notice, width,
    )));
    lines
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
        // Leave the terminal's last column untouched while painting content.
        // Many terminals enable delayed auto-wrap when a printable cell reaches
        // the final column, which can make the right edge look clipped or can
        // spill into the next row before the cursor is moved for the next draw.
        let render_width = terminal_width.saturating_sub(1).max(1);
        let height = usize::from(height).max(1);
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
        let status_index = all_footer_lines.len().saturating_sub(2);
        let (panel_lines, all_status_lines) = all_footer_lines.split_at(status_index);
        let desired_composer = preview_layout.visible_lines.len().saturating_add(1);
        let budget = responsive_frame_budget(height, desired_composer, panel_lines.len());

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
        let visible_body = {
            let rendered_body = self.rendered_body_lines(state, render_width);
            let rendered_body_len = rendered_body.len();
            state.update_render_metrics(rendered_body_len, budget.body);
            let (visible_start, visible_end) =
                visible_body_bounds(state, rendered_body_len, budget.body);
            rendered_body[visible_start..visible_end].to_vec()
        };

        let mut frame_lines = vec![StyledLine::plain(""); height];
        for (row, line) in visible_body.iter().take(budget.body).enumerate() {
            if let Some(slot) = frame_lines.get_mut(row) {
                *slot = line.clone();
            }
        }

        let panel_start = budget.body;
        for (offset, line) in panel_lines.iter().enumerate() {
            if let Some(slot) = frame_lines.get_mut(panel_start.saturating_add(offset)) {
                *slot = line.clone();
            }
        }

        let status_start = panel_start.saturating_add(panel_lines.len());
        for (offset, line) in status_lines.iter().enumerate() {
            if let Some(slot) = frame_lines.get_mut(status_start.saturating_add(offset)) {
                *slot = line.clone();
            }
        }

        let composer_start = status_start.saturating_add(status_lines.len());
        for (offset, line) in composer_lines.iter().enumerate() {
            if let Some(slot) = frame_lines.get_mut(composer_start.saturating_add(offset)) {
                *slot = line.clone();
            }
        }

        self.frame_cache
            .reset_if_geometry_changed(render_width, height);
        let changed_rows = frame_lines
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
                    render_width,
                )?;
                self.frame_cache.set_line(row, line.clone());
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
        let cursor_col = 2usize.saturating_add(composer_layout.cursor_col);
        queue!(
            self.stdout,
            MoveTo(
                u16::try_from(cursor_col.min(render_width.saturating_sub(1))).unwrap_or(u16::MAX),
                u16::try_from(cursor_row).unwrap_or(u16::MAX),
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
        && state.running
    {
        state.request_cancel();
        return Some(InteractiveTuiEvent::Cancel);
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
            if state.running {
                state.show_run_active_hint();
            } else {
                return Some(InteractiveTuiEvent::Quit);
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
        KeyCode::Esc => {
            state.open_selection_mode();
        }
        KeyCode::Tab => {
            state.toggle_enter_mode();
        }
        KeyCode::Enter if !state.enter_sends() => {
            state.insert_composer_newline();
        }
        KeyCode::Enter if state.running => {
            if state.take_paste_image_command() {
                return Some(InteractiveTuiEvent::PasteImage);
            }
            if let Some(steering) = state.take_steering_prompt() {
                state.push_history(steering.text.clone());
                return Some(InteractiveTuiEvent::Steer(steering));
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
                state.push_history(prompt.display_text());
                return Some(InteractiveTuiEvent::Submit(prompt));
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
