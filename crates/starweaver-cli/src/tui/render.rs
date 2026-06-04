use std::{env, io};

use crossterm::{
    cursor::MoveTo,
    queue,
    style::{Attribute, Color, Print, ResetColor, SetAttribute, SetForegroundColor},
};
use serde_json::Value;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{CliError, CliResult};

use super::{markdown::render_transcript_lines, snapshot::TuiSnapshot, state::InteractiveTuiState};

const LIVE_PREFIX_COLS: usize = 2;
const SESSION_HEADER_MAX_INNER_WIDTH: usize = 56;
const STARWEAVER_CLI_VERSION: &str = env!("CARGO_PKG_VERSION");

pub(super) fn snapshot_interactive_lines(snapshot: &TuiSnapshot) -> Vec<String> {
    let mut lines = Vec::new();
    if !snapshot.assistant_text.trim().is_empty() {
        lines.push("Assistant:".to_string());
        lines.extend(snapshot.assistant_text.trim().lines().map(str::to_string));
    }
    if !snapshot.tool_calls.is_empty() {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        for tool in &snapshot.tool_calls {
            lines.push(format!("Tool call: {tool}"));
        }
    }
    if let Some(status) = snapshot.terminal_status.as_deref() {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push(format!(
            "Run completed: {} status={status}",
            snapshot.session_id
        ));
    }
    lines
}

pub(super) fn render_live_history_lines(
    state: &InteractiveTuiState,
    width: usize,
) -> Vec<StyledLine> {
    let mut lines = render_session_header_card(state, width);
    if state.body.is_empty() {
        lines.extend(render_startup_help());
    } else {
        lines.push(StyledLine::plain(""));
        lines.extend(render_transcript_lines(&state.body, width));
    }
    lines
}

fn render_session_header_card(state: &InteractiveTuiState, width: usize) -> Vec<StyledLine> {
    if width < 4 {
        return Vec::new();
    }
    let inner_width = width.saturating_sub(4).min(SESSION_HEADER_MAX_INNER_WIDTH);
    let label_width = "directory:".len();
    let model_label = format!("{:<label_width$} ", "model:");
    let directory_label = format!("{:<label_width$} ", "directory:");
    let directory_width = inner_width.saturating_sub(visible_width(&directory_label));
    let directory = truncate_center_path(&state.workspace_dir, directory_width);

    let rows = vec![
        vec![
            StyledSegment {
                text: ">_ ".to_string(),
                style: SegmentStyle::dim(),
            },
            StyledSegment {
                text: "Starweaver".to_string(),
                style: SegmentStyle::bold(),
            },
            StyledSegment {
                text: " ".to_string(),
                style: SegmentStyle::dim(),
            },
            StyledSegment {
                text: format!("(v{STARWEAVER_CLI_VERSION})"),
                style: SegmentStyle::dim(),
            },
        ],
        Vec::new(),
        vec![
            StyledSegment {
                text: model_label,
                style: SegmentStyle::dim(),
            },
            StyledSegment {
                text: state.model.clone(),
                style: SegmentStyle::default(),
            },
            StyledSegment {
                text: "   ".to_string(),
                style: SegmentStyle::dim(),
            },
            StyledSegment {
                text: "/model".to_string(),
                style: SegmentStyle::code(),
            },
            StyledSegment {
                text: " to change".to_string(),
                style: SegmentStyle::dim(),
            },
        ],
        vec![
            StyledSegment {
                text: directory_label,
                style: SegmentStyle::dim(),
            },
            StyledSegment {
                text: directory,
                style: SegmentStyle::default(),
            },
        ],
    ];
    with_codex_border(rows, inner_width)
}

fn render_startup_help() -> Vec<StyledLine> {
    vec![
        StyledLine::styled(
            "  To get started, describe a task or use these shortcuts:",
            SegmentStyle::dim(),
        ),
        StyledLine::plain(""),
        startup_help_line("Enter", " - submit the current message"),
        startup_help_line("Tab", " - submit, or queue a draft while running"),
        startup_help_line("Ctrl-O", " - insert a newline"),
        startup_help_line("Shift-Tab", " - switch ACT/PLAN mode"),
        startup_help_line("?", " - show keyboard shortcuts"),
    ]
}

fn startup_help_line(key: &str, description: &str) -> StyledLine {
    let mut line = StyledLine::plain("  ");
    line.push(key, SegmentStyle::default());
    line.push(description, SegmentStyle::dim());
    line
}

fn with_codex_border(rows: Vec<Vec<StyledSegment>>, inner_width: usize) -> Vec<StyledLine> {
    let content_width = inner_width;
    let border_inner_width = content_width.saturating_add(2);
    let mut output = Vec::with_capacity(rows.len().saturating_add(2));
    output.push(StyledLine::styled(
        format!("╭{}╮", "─".repeat(border_inner_width)),
        SegmentStyle::dim(),
    ));
    for row in rows {
        let mut used_width = 0usize;
        let mut line = StyledLine::styled("│ ", SegmentStyle::dim());
        for segment in row {
            let remaining = content_width.saturating_sub(used_width);
            if remaining == 0 {
                break;
            }
            let text = truncate_line(&segment.text, remaining);
            used_width = used_width.saturating_add(visible_width(&text));
            line.push(text, segment.style);
        }
        if used_width < content_width {
            line.push(" ".repeat(content_width - used_width), SegmentStyle::dim());
        }
        line.push(" │", SegmentStyle::dim());
        output.push(line);
    }
    output.push(StyledLine::styled(
        format!("╰{}╯", "─".repeat(border_inner_width)),
        SegmentStyle::dim(),
    ));
    output
}

pub(super) fn render_composer_lines(state: &InteractiveTuiState, width: usize) -> Vec<StyledLine> {
    let input_lines = input_tail_lines(&state.input, 3);
    let mut lines = Vec::with_capacity(input_lines.len().saturating_add(2));
    lines.push(StyledLine::plain(""));
    for (offset, input) in input_lines.iter().enumerate() {
        let mut line =
            StyledLine::styled(if offset == 0 { "›" } else { " " }, SegmentStyle::bold());
        line.push(
            " ".repeat(LIVE_PREFIX_COLS.saturating_sub(1)),
            SegmentStyle::default(),
        );
        if input.is_empty() && offset == 0 {
            line.push("Ask Starweaver to do anything", SegmentStyle::dim());
        } else {
            line.push(input, SegmentStyle::default());
        }
        lines.push(pad_styled_line(line, width));
    }
    lines.push(StyledLine::plain(""));
    lines
}

pub(super) fn render_footer_lines(state: &InteractiveTuiState, width: usize) -> Vec<StyledLine> {
    if state.footer_mode.is_shortcuts() {
        render_shortcut_overlay(width)
    } else {
        vec![render_footer_line(state, width)]
    }
}

pub(super) fn render_footer_line(state: &InteractiveTuiState, width: usize) -> StyledLine {
    let right = context_window_line(state);
    let left = footer_left_hint(state);
    let mut line = StyledLine::plain("  ");
    if let Some(left) = left {
        line.push(left, SegmentStyle::dim());
    }
    let left_width = line.visible_width();
    let right_width = visible_width(&right);
    if width > left_width.saturating_add(right_width).saturating_add(1) {
        line.push(
            " ".repeat(width - left_width - right_width),
            SegmentStyle::dim(),
        );
        line.push(right, SegmentStyle::dim());
    }
    pad_styled_line(line, width)
}

fn footer_left_hint(state: &InteractiveTuiState) -> Option<&'static str> {
    if state.running {
        return Some("tab to queue message");
    }
    if !state.input.is_empty() {
        return None;
    }
    Some("? for shortcuts")
}

fn context_window_line(state: &InteractiveTuiState) -> String {
    if state.running {
        format!("{} · {}", state.run_mode.label(), state.phase)
    } else {
        "100% context left".to_string()
    }
}

pub(super) fn render_shortcut_overlay(width: usize) -> Vec<StyledLine> {
    let rows = [
        ("enter to submit message", "tab to submit or queue"),
        ("ctrl + o for newline", "shift + tab to change mode"),
        ("up/down for history", "page up/down to scroll"),
        ("ctrl + r previous prompt", "ctrl + c to interrupt or exit"),
        ("ctrl + l jump to bottom", "ctrl + d to quit"),
        ("", ""),
        ("shortcuts match the active Starweaver TUI", ""),
    ];
    rows.into_iter()
        .map(|(left, right)| {
            let mut line = StyledLine::plain("  ");
            line.push(left, SegmentStyle::dim());
            let left_width = visible_width(left);
            let column_width = 38usize;
            if !right.is_empty() {
                line.push(
                    " ".repeat(column_width.saturating_sub(left_width).saturating_add(4)),
                    SegmentStyle::dim(),
                );
                line.push(right, SegmentStyle::dim());
            }
            pad_styled_line(line, width)
        })
        .collect()
}

fn pad_styled_line(mut line: StyledLine, width: usize) -> StyledLine {
    let line_width = line.visible_width();
    if line_width < width {
        line.push(" ".repeat(width - line_width), SegmentStyle::default());
    }
    line
}

fn truncate_center_path(path: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if visible_width(path) <= width {
        return path.to_string();
    }
    if width <= 1 {
        return "…".to_string();
    }
    let ellipsis_width = visible_width("…");
    let left_width = width.saturating_sub(ellipsis_width) / 2;
    let right_width = width.saturating_sub(ellipsis_width + left_width);
    let left = take_prefix_width(path, left_width);
    let right = take_suffix_width(path, right_width);
    format!("{left}…{right}")
}

pub(super) fn visible_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

fn take_prefix_width(text: &str, width: usize) -> String {
    let mut output = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let char_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used.saturating_add(char_width) > width {
            break;
        }
        output.push(ch);
        used = used.saturating_add(char_width);
    }
    output
}

fn take_suffix_width(text: &str, width: usize) -> String {
    let mut chars = Vec::new();
    let mut used = 0usize;
    for ch in text.chars().rev() {
        let char_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used.saturating_add(char_width) > width {
            break;
        }
        chars.push(ch);
        used = used.saturating_add(char_width);
    }
    chars.into_iter().rev().collect()
}

pub(super) fn composer_cursor_column(input_tail: &[String]) -> usize {
    let current = input_tail.last().map_or("", String::as_str);
    LIVE_PREFIX_COLS.saturating_add(visible_width(current))
}

pub(super) fn input_tail_lines(input: &str, max_lines: usize) -> Vec<String> {
    let mut lines = input.lines().map(str::to_string).collect::<Vec<_>>();
    if input.ends_with('\n') || lines.is_empty() {
        lines.push(String::new());
    }
    let start = lines.len().saturating_sub(max_lines.max(1));
    lines.into_iter().skip(start).collect()
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct SegmentStyle(u8);

impl SegmentStyle {
    pub(super) const BOLD: u8 = 0b00_0001;
    pub(super) const ITALIC: u8 = 0b00_0010;
    pub(super) const UNDERLINED: u8 = 0b00_0100;
    pub(super) const DIM: u8 = 0b00_1000;
    pub(super) const CYAN: u8 = 0b01_0000;
    pub(super) const GREEN: u8 = 0b10_0000;
    pub(super) const BLUE: u8 = 0b100_0000;

    pub(super) const fn bold() -> Self {
        Self(Self::BOLD)
    }

    pub(super) const fn italic() -> Self {
        Self(Self::ITALIC)
    }

    pub(super) const fn underlined() -> Self {
        Self(Self::UNDERLINED)
    }

    pub(super) const fn dim() -> Self {
        Self(Self::DIM)
    }

    pub(super) const fn code() -> Self {
        Self(Self::CYAN)
    }

    pub(super) const fn code_block() -> Self {
        Self(Self::CYAN)
    }

    pub(super) const fn link() -> Self {
        Self(Self::CYAN | Self::UNDERLINED)
    }

    pub(super) const fn blockquote() -> Self {
        Self(Self::GREEN)
    }

    pub(super) const fn list_marker() -> Self {
        Self(Self::BLUE)
    }

    pub(super) const fn merge(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub(super) const fn contains(self, flag: u8) -> bool {
        self.0 & flag != 0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct StyledSegment {
    pub(super) text: String,
    pub(super) style: SegmentStyle,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct StyledLine {
    pub(super) segments: Vec<StyledSegment>,
}

impl StyledLine {
    pub(super) fn plain(text: impl Into<String>) -> Self {
        Self {
            segments: vec![StyledSegment {
                text: text.into(),
                style: SegmentStyle::default(),
            }],
        }
    }

    pub(super) fn styled(text: impl Into<String>, style: SegmentStyle) -> Self {
        Self {
            segments: vec![StyledSegment {
                text: text.into(),
                style,
            }],
        }
    }

    pub(super) fn push(&mut self, text: impl Into<String>, style: SegmentStyle) {
        let text = text.into();
        if text.is_empty() {
            return;
        }
        self.segments.push(StyledSegment { text, style });
    }

    pub(super) fn visible_width(&self) -> usize {
        self.segments
            .iter()
            .map(|segment| visible_width(&segment.text))
            .sum()
    }
}

pub(super) fn queue_styled_line(
    stdout: &mut io::Stdout,
    line: &StyledLine,
    width: usize,
) -> CliResult<()> {
    queue_styled_segments(stdout, line, width)?;
    queue!(stdout, Print("\r\n")).map_err(terminal_error)
}

pub(super) fn queue_styled_line_at(
    stdout: &mut io::Stdout,
    row: u16,
    line: &StyledLine,
    width: usize,
) -> CliResult<()> {
    queue!(stdout, MoveTo(0, row)).map_err(terminal_error)?;
    queue_styled_segments(stdout, line, width)
}

fn queue_styled_segments(
    stdout: &mut io::Stdout,
    line: &StyledLine,
    width: usize,
) -> CliResult<()> {
    let mut remaining = width;
    for segment in &line.segments {
        if remaining == 0 {
            break;
        }
        let text = truncate_line(&segment.text, remaining);
        remaining = remaining.saturating_sub(visible_width(&text));
        queue_segment_style(stdout, segment.style)?;
        queue!(
            stdout,
            Print(text),
            SetAttribute(Attribute::Reset),
            ResetColor
        )
        .map_err(terminal_error)?;
    }
    if remaining > 0 {
        queue!(stdout, Print(" ".repeat(remaining))).map_err(terminal_error)?;
    }
    Ok(())
}

fn queue_segment_style(stdout: &mut io::Stdout, style: SegmentStyle) -> CliResult<()> {
    if style.contains(SegmentStyle::BOLD) {
        queue!(stdout, SetAttribute(Attribute::Bold)).map_err(terminal_error)?;
    }
    if style.contains(SegmentStyle::ITALIC) {
        queue!(stdout, SetAttribute(Attribute::Italic)).map_err(terminal_error)?;
    }
    if style.contains(SegmentStyle::UNDERLINED) {
        queue!(stdout, SetAttribute(Attribute::Underlined)).map_err(terminal_error)?;
    }
    if style.contains(SegmentStyle::DIM) {
        queue!(stdout, SetAttribute(Attribute::Dim)).map_err(terminal_error)?;
    }
    if style.contains(SegmentStyle::CYAN) {
        queue!(stdout, SetForegroundColor(Color::Cyan)).map_err(terminal_error)?;
    } else if style.contains(SegmentStyle::GREEN) {
        queue!(stdout, SetForegroundColor(Color::Green)).map_err(terminal_error)?;
    } else if style.contains(SegmentStyle::BLUE) {
        queue!(stdout, SetForegroundColor(Color::Blue)).map_err(terminal_error)?;
    }
    Ok(())
}

pub(super) fn value_preview(value: &Value) -> String {
    let text = match value {
        Value::String(value) => value.clone(),
        other => other.to_string(),
    };
    let compact = text.replace('\n', " ");
    truncate_line(&compact, 80)
}

pub(super) fn truncate_line(line: &str, width: usize) -> String {
    take_prefix_width(line, width)
}

pub(super) fn terminal_error(error: impl std::fmt::Display) -> CliError {
    CliError::Run(format!("terminal UI failed: {error}"))
}
