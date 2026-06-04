use std::{
    io::{self, Write},
    time::{Duration, Instant},
};

use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute, queue,
    terminal::{self, Clear, ClearType},
};

use crate::CliResult;

use super::{
    render::{
        composer_cursor_column, input_tail_lines, queue_styled_line, queue_styled_line_at,
        render_composer_lines, render_footer_lines, render_live_history_lines, terminal_error,
    },
    state::{FooterMode, InteractiveTuiState, RunMode},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InteractiveTuiEvent {
    /// Submit a prompt.
    Submit(String),
    /// Queue a prompt while a run is active.
    Queue(String),
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
        execute!(stdout, Hide).map_err(terminal_error)?;
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
        queue!(self.stdout, Clear(ClearType::All), MoveTo(0, 0)).map_err(terminal_error)?;

        let composer_lines = render_composer_lines(state, width);
        let footer_lines = render_footer_lines(state, width);
        let bottom_height = composer_lines.len().saturating_add(footer_lines.len());
        let body_height = height.saturating_sub(bottom_height).max(1);
        let rendered_body = render_live_history_lines(state, width);
        let visible_start = rendered_body
            .len()
            .saturating_sub(body_height.saturating_add(state.scroll_offset));
        let visible_end = rendered_body
            .len()
            .saturating_sub(state.scroll_offset)
            .max(visible_start);

        for line in rendered_body
            .iter()
            .skip(visible_start)
            .take(visible_end.saturating_sub(visible_start))
        {
            queue_styled_line(&mut self.stdout, line, width)?;
        }

        let composer_start = height.saturating_sub(bottom_height);
        for (offset, line) in composer_lines.iter().enumerate() {
            queue_styled_line_at(
                &mut self.stdout,
                u16::try_from(composer_start.saturating_add(offset)).unwrap_or(u16::MAX),
                line,
                width,
            )?;
        }
        let footer_start = composer_start.saturating_add(composer_lines.len());
        for (offset, line) in footer_lines.iter().enumerate() {
            queue_styled_line_at(
                &mut self.stdout,
                u16::try_from(footer_start.saturating_add(offset)).unwrap_or(u16::MAX),
                line,
                width,
            )?;
        }
        let input_tail = input_tail_lines(&state.input, 3);
        let cursor_row = composer_start
            .saturating_add(1)
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
        let Event::Key(key) = event::read().map_err(terminal_error)? else {
            return Ok(None);
        };
        Ok(handle_key_event(state, key))
    }
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
            state.scroll_offset = 0;
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.input.clear();
            state.history_index = None;
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
        KeyCode::Char('q') if state.input.is_empty() => {
            if state.running {
                state.show_run_active_hint();
            } else {
                return Some(InteractiveTuiEvent::Quit);
            }
        }
        KeyCode::Char('?') if state.input.is_empty() && !state.running => {
            state.footer_mode.toggle_shortcuts();
        }
        KeyCode::BackTab => {
            state.run_mode = match state.run_mode {
                RunMode::Act => RunMode::Plan,
                RunMode::Plan => RunMode::Act,
            };
        }
        KeyCode::Tab if state.running => {
            let prompt = state.input.trim().to_string();
            state.input.clear();
            if !prompt.is_empty() {
                state.push_history(prompt.clone());
                return Some(InteractiveTuiEvent::Queue(prompt));
            }
        }
        KeyCode::Tab | KeyCode::Enter => {
            let prompt = state.input.trim().to_string();
            state.input.clear();
            if !prompt.is_empty() {
                state.push_history(prompt.clone());
                return Some(InteractiveTuiEvent::Submit(prompt));
            }
        }
        KeyCode::Backspace => {
            state.input.pop();
        }
        KeyCode::PageUp => {
            state.scroll_offset = state.scroll_offset.saturating_add(10).min(state.body.len());
        }
        KeyCode::PageDown => {
            state.scroll_offset = state.scroll_offset.saturating_sub(10);
        }
        KeyCode::Up if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.scroll_offset = state.scroll_offset.saturating_add(1).min(state.body.len());
        }
        KeyCode::Down if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.scroll_offset = state.scroll_offset.saturating_sub(1);
        }
        KeyCode::Up => state.previous_history(),
        KeyCode::Down => state.next_history(),
        KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.input.push(ch);
            state.history_index = None;
            state.footer_mode = FooterMode::Context;
        }
        _ => {}
    }
    None
}

impl Drop for InteractiveTui {
    fn drop(&mut self) {
        if self.active {
            let _ = execute!(self.stdout, Show);
            let _ = terminal::disable_raw_mode();
            self.active = false;
        }
    }
}
