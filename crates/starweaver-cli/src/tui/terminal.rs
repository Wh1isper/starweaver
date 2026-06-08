use std::{
    io::{self, Write},
    time::{Duration, Instant},
};

use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{
        self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers,
    },
    execute, queue,
    terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};

use crate::CliResult;

use super::{
    render::{
        composer_cursor_column, input_tail_lines, queue_styled_line_at, render_composer_lines,
        render_footer_lines, render_live_history_lines, terminal_error,
    },
    state::{InteractiveTuiState, RunMode, SteeringSubmission},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InteractiveTuiEvent {
    /// Redraw after a handled key changed or may have changed local UI state.
    Redraw,
    /// Submit a prompt.
    Submit(String),
    /// Queue a prompt while a run is active.
    Queue(String),
    /// Send steering to the active run UI pane.
    Steer(SteeringSubmission),
    /// Interrupt the active run.
    Cancel,
    /// Quit the TUI.
    Quit,
}

/// Interactive terminal UI session.
pub struct InteractiveTui {
    stdout: io::Stdout,
    active: bool,
}

impl InteractiveTui {
    /// Enter Codex-style inline interactive mode.
    pub fn enter() -> CliResult<Self> {
        let mut stdout = io::stdout();
        terminal::enable_raw_mode().map_err(terminal_error)?;
        if let Err(error) = execute!(stdout, EnterAlternateScreen, EnableBracketedPaste, Hide) {
            let _ = terminal::disable_raw_mode();
            return Err(terminal_error(error));
        }
        Ok(Self {
            stdout,
            active: true,
        })
    }

    /// Render the current state.
    pub fn render(&mut self, state: &InteractiveTuiState) -> CliResult<()> {
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
        let input_tail = input_tail_lines(&state.input, 3);
        let cursor_row = composer_start
            .saturating_add(1)
            .saturating_add(state.pasted_image_count())
            .saturating_add(input_tail.len().saturating_sub(1));
        let cursor_col = composer_cursor_column(&input_tail);
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
            Event::Resize(_, _) => Ok(Some(InteractiveTuiEvent::Redraw)),
            _ => Ok(None),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ScrollDirection {
    Up,
    Down,
}

fn scroll_viewport(state: &mut InteractiveTuiState, amount: usize, direction: ScrollDirection) {
    let (width, height) = terminal::size().unwrap_or((80, 24));
    let width = if width == 0 { 80 } else { width };
    let height = if height == 0 { 24 } else { height };
    let width = usize::from(width);
    let height = usize::from(height).max(8);
    let fixed_height = render_composer_lines(state, width)
        .len()
        .saturating_add(render_footer_lines(state, width).len());
    let body_height = height.saturating_sub(fixed_height).max(1);
    let rendered_body_len = render_live_history_lines(state, width).len();
    let max_scroll = rendered_body_len.saturating_sub(body_height);
    let current = if state.is_at_bottom() {
        max_scroll
    } else {
        state.scroll_offset.min(max_scroll)
    };
    let next = match direction {
        ScrollDirection::Up => current.saturating_sub(amount),
        ScrollDirection::Down => current.saturating_add(amount),
    };
    if next >= max_scroll {
        state.scroll_to_bottom();
    } else {
        state.scroll_offset = next;
    }
}

fn viewport_is_scrollable(state: &InteractiveTuiState) -> bool {
    let (width, height) = terminal::size().unwrap_or((80, 24));
    let width = if width == 0 { 80 } else { width };
    let height = if height == 0 { 24 } else { height };
    let width = usize::from(width);
    let height = usize::from(height).max(8);
    let fixed_height = render_composer_lines(state, width)
        .len()
        .saturating_add(render_footer_lines(state, width).len());
    let body_height = height.saturating_sub(fixed_height).max(1);
    render_live_history_lines(state, width).len() > body_height
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
    match key.code {
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if state.running {
                state.request_cancel();
                return Some(InteractiveTuiEvent::Cancel);
            }
            let now = Instant::now();
            let should_exit = state
                .last_ctrl_c
                .is_some_and(|last| now.duration_since(last) < Duration::from_millis(900));
            state.last_ctrl_c = Some(now);
            if should_exit || state.input.is_empty() {
                return Some(InteractiveTuiEvent::Quit);
            }
            state.input.clear();
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
        KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.input.push('\n');
        }
        KeyCode::Char('p' | 'r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.previous_history();
        }
        KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.next_history();
        }
        KeyCode::Esc => {
            if state.running {
                state.show_run_active_hint();
            } else {
                return Some(InteractiveTuiEvent::Quit);
            }
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
        KeyCode::Tab if state.running => {
            if let Some(prompt) = state.take_queued_prompt() {
                state.push_history(prompt.clone());
                return Some(InteractiveTuiEvent::Queue(prompt));
            }
        }
        KeyCode::Enter if state.running => {
            if let Some(steering) = state.take_steering_prompt() {
                state.push_history(steering.text.clone());
                return Some(InteractiveTuiEvent::Steer(steering));
            }
        }
        KeyCode::Tab | KeyCode::Enter => {
            if let Some(prompt) = state.take_submission_prompt() {
                state.push_history(prompt.clone());
                return Some(InteractiveTuiEvent::Submit(prompt));
            }
        }
        KeyCode::Backspace => {
            state.backspace_composer();
        }
        KeyCode::PageUp => {
            scroll_viewport(state, 10, ScrollDirection::Up);
        }
        KeyCode::PageDown => {
            scroll_viewport(state, 10, ScrollDirection::Down);
        }
        KeyCode::Up if key.modifiers.contains(KeyModifiers::CONTROL) => {
            scroll_viewport(state, 1, ScrollDirection::Up);
        }
        KeyCode::Down if key.modifiers.contains(KeyModifiers::CONTROL) => {
            scroll_viewport(state, 1, ScrollDirection::Down);
        }
        KeyCode::Up if state.composer_is_empty() && viewport_is_scrollable(state) => {
            scroll_viewport(state, 1, ScrollDirection::Up);
        }
        KeyCode::Down if state.composer_is_empty() && viewport_is_scrollable(state) => {
            scroll_viewport(state, 1, ScrollDirection::Down);
        }
        KeyCode::Up => state.previous_history(),
        KeyCode::Down => state.next_history(),
        KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.input.push(ch);
            state.input_status = None;
            state.history_index = None;
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
                DisableBracketedPaste,
                LeaveAlternateScreen
            );
            let _ = terminal::disable_raw_mode();
            self.active = false;
        }
    }
}
