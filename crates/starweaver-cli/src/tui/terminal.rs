use std::{
    io::{self, Write},
    time::{Duration, Instant},
};

use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent, MouseEventKind,
    },
    execute, queue,
    terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};

use crate::{prompt_input::PromptInput, CliResult};

use super::{
    render::{
        composer_cursor_position, input_viewport_lines, queue_styled_line_at,
        render_composer_lines, render_footer_lines, render_live_history_lines, terminal_error,
    },
    state::{
        BodyScrollDirection, InteractiveTuiState, PendingSessionCommand, RunMode,
        SteeringSubmission, COMPOSER_VISIBLE_LINES,
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
            let _ = terminal::disable_raw_mode();
            return Err(terminal_error(error));
        }
        Ok(Self {
            stdout,
            active: true,
            mouse_capture_enabled: true,
        })
    }

    /// Render the current state.
    pub fn render(&mut self, state: &mut InteractiveTuiState) -> CliResult<()> {
        self.sync_mouse_capture(should_capture_mouse(state))?;
        let (width, height) = terminal::size().unwrap_or((80, 24));
        let width = if width == 0 { 80 } else { width };
        let height = if height == 0 { 24 } else { height };
        let width = usize::from(width);
        let height = usize::from(height).max(8);
        let composer_lines = render_composer_lines(state, width);
        let status_lines = render_footer_lines(state, width);
        let fixed_height = composer_lines.len().saturating_add(status_lines.len());
        let body_height = height.saturating_sub(fixed_height).max(1);
        let rendered_body = render_live_history_lines(state, width);
        state.update_render_metrics(rendered_body.len(), body_height);
        let (visible_start, visible_end) =
            visible_body_bounds(state, rendered_body.len(), body_height);

        let visible_body = rendered_body
            .iter()
            .skip(visible_start)
            .take(visible_end.saturating_sub(visible_start))
            .collect::<Vec<_>>();
        for row in 0..body_height {
            queue!(
                self.stdout,
                MoveTo(0, u16::try_from(row).unwrap_or(u16::MAX)),
                Clear(ClearType::CurrentLine)
            )
            .map_err(terminal_error)?;
            if let Some(line) = visible_body.get(row) {
                queue_styled_line_at(
                    &mut self.stdout,
                    u16::try_from(row).unwrap_or(u16::MAX),
                    line,
                    width,
                )?;
            }
        }

        let status_start = height.saturating_sub(fixed_height);
        for (offset, line) in status_lines.iter().enumerate() {
            queue_styled_line_at(
                &mut self.stdout,
                u16::try_from(status_start.saturating_add(offset)).unwrap_or(u16::MAX),
                line,
                width,
            )?;
        }

        let composer_start = status_start.saturating_add(status_lines.len());
        for (offset, line) in composer_lines.iter().enumerate() {
            queue_styled_line_at(
                &mut self.stdout,
                u16::try_from(composer_start.saturating_add(offset)).unwrap_or(u16::MAX),
                line,
                width,
            )?;
        }
        let input_tail = input_viewport_lines(
            &state.input,
            COMPOSER_VISIBLE_LINES,
            state.composer_scroll_offset(),
        );
        let total_input_lines = input_line_count(&state.input);
        let max_start = total_input_lines.saturating_sub(COMPOSER_VISIBLE_LINES);
        let visible_start = max_start.saturating_sub(state.composer_scroll_offset().min(max_start));
        let (cursor_line, cursor_col) =
            composer_cursor_position(&state.input, state.composer_cursor_byte());
        let cursor_row = composer_start.saturating_add(1).saturating_add(
            cursor_line
                .saturating_sub(visible_start)
                .min(input_tail.len().saturating_sub(1)),
        );
        queue!(
            self.stdout,
            MoveTo(
                u16::try_from(cursor_col.min(width.saturating_sub(1))).unwrap_or(u16::MAX),
                u16::try_from(cursor_row).unwrap_or(u16::MAX),
            ),
            Show
        )
        .map_err(terminal_error)?;
        self.stdout.flush().map_err(terminal_error)
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

fn input_line_count(input: &str) -> usize {
    let count = input.lines().count();
    if input.ends_with('\n') || count == 0 {
        count.saturating_add(1)
    } else {
        count
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
            let now = Instant::now();
            let should_exit = state
                .last_ctrl_c
                .is_some_and(|last| now.duration_since(last) < Duration::from_millis(900));
            state.last_ctrl_c = Some(now);
            if should_exit || state.input.is_empty() {
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
        KeyCode::Char('p' | 'r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.previous_history();
        }
        KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.next_history();
        }
        KeyCode::Esc => {
            state.open_selection_mode();
        }
        KeyCode::Char('q') if state.composer_is_empty() => {
            if state.running {
                state.show_run_active_hint();
            } else {
                return Some(InteractiveTuiEvent::Quit);
            }
        }
        KeyCode::BackTab => {
            state.run_mode = match state.run_mode {
                RunMode::Act => RunMode::Plan,
                RunMode::Plan => RunMode::Act,
            };
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
        KeyCode::Up => state.previous_history(),
        KeyCode::Down => state.next_history(),
        KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.push_composer_char(ch);
        }
        _ => {}
    }
    None
}

impl Drop for InteractiveTui {
    fn drop(&mut self) {
        if self.active {
            let _ = execute!(
                self.stdout,
                Show,
                DisableMouseCapture,
                DisableBracketedPaste,
                LeaveAlternateScreen
            );
            let _ = terminal::disable_raw_mode();
            self.active = false;
        }
    }
}
