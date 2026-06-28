use std::{env, io};

use crossterm::{
    cursor::MoveTo,
    queue,
    style::{
        Attribute, Color, Print, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor,
    },
    terminal::{Clear, ClearType},
};
use serde_json::Value;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{CliError, CliResult};

mod panels;
mod pickers;

use panels::{
    render_hitl_panel, render_selection_panel, render_status_bar_lines, render_task_panel,
};
use pickers::{push_detail_row, render_model_picker_panel, render_session_picker_panel};

use super::{
    markdown::{render_transcript_lines, ASSISTANT_CONTENT_PREFIX},
    snapshot::TuiSnapshot,
    state::{HitlPanelState, InteractiveTuiState, TaskPanelItem},
};

const SESSION_HEADER_MAX_INNER_WIDTH: usize = 56;
const STARWEAVER_CLI_VERSION: &str = env!("CARGO_PKG_VERSION");

pub(super) fn snapshot_interactive_lines(snapshot: &TuiSnapshot) -> Vec<String> {
    if !snapshot.transcript_lines.is_empty() {
        let mut lines = snapshot.transcript_lines.clone();
        if let Some(status) = snapshot.terminal_status.as_deref() {
            if !lines.is_empty() {
                lines.push(String::new());
            }
            lines.push(format!(
                "Run completed: {} status={status}",
                snapshot.session_id
            ));
        }
        return lines;
    }
    let mut lines = Vec::new();
    if !snapshot.assistant_text.trim().is_empty() {
        lines.push("Assistant:".to_string());
        lines.extend(
            snapshot
                .assistant_text
                .trim()
                .lines()
                .map(|line| format!("{ASSISTANT_CONTENT_PREFIX}{line}")),
        );
    }
    if !snapshot.tool_calls.is_empty() {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        for tool in &snapshot.tool_calls {
            if let Some(result) = tool.strip_prefix("result:error:") {
                lines.push(format!("Tool error: {result}"));
            } else if let Some(result) = tool.strip_prefix("result:") {
                lines.push(format!("Tool result: {result}"));
            } else {
                lines.push(format!("Tool call: {tool}"));
            }
        }
    }
    if !snapshot.steering.is_empty() {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        for item in &snapshot.steering {
            if let Some(text) = item.strip_prefix("received:") {
                lines.push(format!("Steering received: {text}"));
            } else if let Some(text) = item.strip_prefix("submitted:") {
                lines.push(format!("Steering: {text}"));
            } else {
                lines.push(format!("Steering: {item}"));
            }
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
        startup_help_line("Tab", " - toggle whether Enter sends or inserts a newline"),
        startup_help_line("Ctrl-O", " - insert a newline"),
        startup_help_line("/help", " - print available commands"),
        startup_help_line("/model", " - open the model profile selector"),
        startup_help_line("/session", " - open the session selector"),
        startup_help_line("!<command>", " - run a shell command inline"),
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

#[cfg(test)]
pub(super) fn render_composer_lines(state: &InteractiveTuiState, width: usize) -> Vec<StyledLine> {
    let input_width = composer_input_width(width);
    let layout = composer_layout(
        &state.input,
        state.composer_cursor_byte(),
        super::state::COMPOSER_VISIBLE_LINES,
        state.composer_scroll_offset(),
        input_width,
    );
    render_composer_lines_from_layout(state, width, &layout)
}

pub(super) fn render_composer_lines_from_layout(
    state: &InteractiveTuiState,
    width: usize,
    layout: &ComposerLayout,
) -> Vec<StyledLine> {
    let mut lines = Vec::with_capacity(layout.visible_lines.len().saturating_add(1));
    lines.push(StyledLine::plain(""));
    let prompt = composer_prompt(state);
    for (offset, input) in layout.visible_lines.iter().enumerate() {
        let mut line =
            StyledLine::styled(if offset == 0 { prompt } else { " " }, SegmentStyle::bold());
        line.push(" ", SegmentStyle::default());
        if input.is_empty() && offset == 0 && state.input.is_empty() {
            line.push(composer_placeholder(state), SegmentStyle::dim());
        } else {
            line.push(input, SegmentStyle::default());
        }
        lines.push(pad_styled_line(line, width));
    }
    lines
}

const fn composer_prompt(state: &InteractiveTuiState) -> &'static str {
    if state.running {
        "*"
    } else {
        ">"
    }
}

const fn composer_placeholder(state: &InteractiveTuiState) -> &'static str {
    if state.running {
        "Steer the running task"
    } else {
        "Ask Starweaver to do anything"
    }
}

pub(super) fn render_footer_lines(state: &InteractiveTuiState, width: usize) -> Vec<StyledLine> {
    let mut lines = if InteractiveTuiState::help_panel_visible() {
        render_help_panel(width)
    } else {
        Vec::new()
    };
    if state.session_picker_visible() {
        lines.extend(render_session_picker_panel(state, width));
    } else if state.model_picker_visible() {
        lines.extend(render_model_picker_panel(state, width));
    } else if state.selection_mode_visible() {
        lines.extend(render_selection_panel(state, width));
    }
    if let Some(hitl) = state.pending_hitl() {
        lines.extend(render_hitl_panel(hitl, width));
    }
    if !state.task_panel_items().is_empty() {
        lines.extend(render_task_panel(state.task_panel_items(), width));
    }
    lines.extend(render_status_bar_lines(state, width));
    lines
}

#[cfg(test)]
pub(super) fn render_shortcut_overlay(width: usize) -> Vec<StyledLine> {
    render_help_panel(width)
}

pub(super) fn render_help_panel(width: usize) -> Vec<StyledLine> {
    let command_rows = [
        ("/help", "Print this help in the transcript"),
        ("/clear", "Clear output and start a fresh context"),
        ("/cost", "Show usage and cost summary"),
        ("/model [profile]", "Open selector or select model profile"),
        ("/session [id]", "Open selector or reload session"),
        (
            "/goal <task>",
            "Run task toward a verified goal until complete",
        ),
        ("/paste-image", "Attach image from system clipboard"),
    ];
    let key_rows = [
        ("Ctrl+C", "Interrupt active run or exit"),
        ("Ctrl+D", "Exit"),
        ("Ctrl+V", "Attach image from system clipboard"),
        ("Tab", "Toggle Enter between send and newline"),
        ("Ctrl+O", "Insert newline"),
        ("Up/Down, Ctrl+P/N", "Browse prompt history"),
        ("Ctrl+A/E", "Move to line start/end"),
        ("Alt+Left/Right", "Move by word"),
        ("Command+Left/Right", "Move to line start/end"),
        ("Alt+Up/Down", "Scroll multiline input"),
        ("PageUp/PageDown", "Scroll transcript"),
        ("Mouse wheel", "Scroll transcript"),
    ];
    let mut lines = Vec::new();
    lines.push(StyledLine::plain(""));
    lines.extend(render_help_heading(
        "Available Commands",
        width,
        SegmentStyle::code().merge(SegmentStyle::bold()),
    ));
    for (command, description) in command_rows {
        lines.extend(render_help_table_rows(
            command,
            description,
            SegmentStyle::blockquote(),
            width,
        ));
    }
    lines.push(StyledLine::plain(""));
    lines.extend(render_help_heading(
        "Shell",
        width,
        SegmentStyle::code().merge(SegmentStyle::bold()),
    ));
    lines.extend(render_help_table_rows(
        "!<command>",
        "Run a shell command and show output inline",
        SegmentStyle::warning(),
        width,
    ));
    lines.push(StyledLine::plain(""));
    lines.extend(render_help_heading(
        "Key Bindings",
        width,
        SegmentStyle::code().merge(SegmentStyle::bold()),
    ));
    for (key, description) in key_rows {
        lines.extend(render_help_table_rows(
            key,
            description,
            SegmentStyle::warning(),
            width,
        ));
    }
    lines
}

fn render_help_heading(text: &str, width: usize, style: SegmentStyle) -> Vec<StyledLine> {
    wrap_text_width(text, width)
        .into_iter()
        .map(|line| pad_styled_line(StyledLine::styled(line, style), width))
        .collect()
}

fn render_help_table_rows(
    label: &str,
    description: &str,
    label_style: SegmentStyle,
    width: usize,
) -> Vec<StyledLine> {
    let width = width.max(1);
    let label_indent = "  ";
    let label_width = 20usize;
    let used = visible_width(label_indent).saturating_add(visible_width(label));
    let description_start = label_width.max(used.saturating_add(2));

    if description_start < width {
        let description_width = width.saturating_sub(description_start).max(1);
        let wrapped_description = wrap_text_width(description, description_width);
        let mut rows = Vec::with_capacity(wrapped_description.len().max(1));
        for (index, description_line) in wrapped_description.into_iter().enumerate() {
            let mut line = if index == 0 {
                let mut line = StyledLine::plain(label_indent);
                line.push(label, label_style);
                line.push(
                    " ".repeat(description_start.saturating_sub(used)),
                    SegmentStyle::dim(),
                );
                line
            } else {
                StyledLine::plain(" ".repeat(description_start))
            };
            line.push(description_line, SegmentStyle::default());
            rows.push(pad_styled_line(line, width));
        }
        rows
    } else {
        let mut rows = wrap_text_width(label, width.saturating_sub(2).max(1))
            .into_iter()
            .map(|label_line| {
                let mut line = StyledLine::plain(label_indent);
                line.push(label_line, label_style);
                pad_styled_line(line, width)
            })
            .collect::<Vec<_>>();
        let description_indent = if width > 4 { "    " } else { "  " };
        let description_width = width
            .saturating_sub(visible_width(description_indent))
            .max(1);
        rows.extend(
            wrap_text_width(description, description_width)
                .into_iter()
                .map(|description_line| {
                    let mut line = StyledLine::plain(description_indent);
                    line.push(description_line, SegmentStyle::default());
                    pad_styled_line(line, width)
                }),
        );
        rows
    }
}

fn pad_styled_line(line: StyledLine, width: usize) -> StyledLine {
    pad_styled_line_with_style(line, width, SegmentStyle::default())
}

fn pad_styled_line_with_style(
    mut line: StyledLine,
    width: usize,
    style: SegmentStyle,
) -> StyledLine {
    let line_width = line.visible_width();
    if line_width < width {
        line.push(" ".repeat(width - line_width), style);
    }
    line
}

fn compact_timestamp(timestamp: &str) -> String {
    timestamp
        .chars()
        .take(19)
        .collect::<String>()
        .replace('T', " ")
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

pub(super) fn take_prefix_width(text: &str, width: usize) -> String {
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

pub(super) fn wrap_text_width(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let compact = text.replace('\n', " ");
    if compact.is_empty() || visible_width(&compact) <= width {
        return vec![compact];
    }
    let mut lines = Vec::new();
    let mut remaining = compact.as_str();
    while !remaining.is_empty() {
        let line = take_prefix_width(remaining, width);
        if line.is_empty() {
            break;
        }
        remaining = &remaining[line.len()..];
        lines.push(line);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

pub(super) fn composer_input_width(total_width: usize) -> usize {
    total_width.saturating_sub(2).max(1)
}

#[cfg(test)]
pub(super) fn composer_cursor_column(input_tail: &[String]) -> usize {
    let current = input_tail.last().map_or("", String::as_str);
    "> ".len().saturating_add(visible_width(current))
}

#[cfg(test)]
#[allow(dead_code)]
pub(super) fn composer_cursor_position(input: &str, cursor_byte: usize) -> (usize, usize) {
    let cursor_byte = clamp_char_boundary(input, cursor_byte);
    let before_cursor = &input[..cursor_byte];
    let line_index = before_cursor.bytes().filter(|byte| *byte == b'\n').count();
    let line_start = before_cursor.rfind('\n').map_or(0, |index| index + 1);
    let column = "> "
        .len()
        .saturating_add(visible_width(&before_cursor[line_start..]));
    (line_index, column)
}

pub(super) fn composer_cursor_position_wrapped(
    input: &str,
    cursor_byte: usize,
    content_width: usize,
) -> (usize, usize) {
    let cursor_byte = clamp_char_boundary(input, cursor_byte);
    wrapped_cursor_position(&input[..cursor_byte], content_width.max(1))
}

#[cfg(test)]
pub(super) fn input_tail_lines(input: &str, max_lines: usize) -> Vec<String> {
    input_viewport_lines(input, max_lines, 0)
}

#[cfg(test)]
pub(super) fn input_viewport_lines(
    input: &str,
    max_lines: usize,
    scroll_from_bottom: usize,
) -> Vec<String> {
    let lines = logical_input_lines(input);
    viewport_from_lines(lines, max_lines, scroll_from_bottom)
}

#[cfg(test)]
pub(super) fn input_visual_line_count(input: &str, content_width: usize) -> usize {
    visual_input_lines(input, content_width).len()
}

#[cfg(test)]
pub(super) fn input_viewport_lines_wrapped(
    input: &str,
    max_lines: usize,
    scroll_from_bottom: usize,
    content_width: usize,
) -> Vec<String> {
    composer_layout(
        input,
        input.len(),
        max_lines,
        scroll_from_bottom,
        content_width,
    )
    .visible_lines
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ComposerLayout {
    pub(super) visible_lines: Vec<String>,
    pub(super) total_visual_lines: usize,
    pub(super) visible_start: usize,
    pub(super) cursor_line: usize,
    pub(super) cursor_col: usize,
}

pub(super) fn composer_layout(
    input: &str,
    cursor_byte: usize,
    max_lines: usize,
    scroll_from_bottom: usize,
    content_width: usize,
) -> ComposerLayout {
    let width = content_width.max(1);
    let lines = visual_input_lines(input, width);
    let total_visual_lines = lines.len();
    let max_lines = max_lines.max(1);
    let max_start = total_visual_lines.saturating_sub(max_lines);
    let visible_start = max_start.saturating_sub(scroll_from_bottom.min(max_start));
    let visible_lines = lines
        .into_iter()
        .skip(visible_start)
        .take(max_lines)
        .collect();
    let (cursor_line, cursor_col) = composer_cursor_position_wrapped(input, cursor_byte, width);
    ComposerLayout {
        visible_lines,
        total_visual_lines,
        visible_start,
        cursor_line,
        cursor_col,
    }
}

fn logical_input_lines(input: &str) -> Vec<String> {
    let mut lines = input.lines().map(str::to_string).collect::<Vec<_>>();
    if input.ends_with('\n') || lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

#[cfg(test)]
fn viewport_from_lines(
    lines: Vec<String>,
    max_lines: usize,
    scroll_from_bottom: usize,
) -> Vec<String> {
    let max_lines = max_lines.max(1);
    let max_start = lines.len().saturating_sub(max_lines);
    let start = max_start.saturating_sub(scroll_from_bottom.min(max_start));
    lines.into_iter().skip(start).take(max_lines).collect()
}

fn visual_input_lines(input: &str, content_width: usize) -> Vec<String> {
    let width = content_width.max(1);
    logical_input_lines(input)
        .into_iter()
        .flat_map(|line| wrap_input_line(&line, width))
        .collect()
}

fn wrap_input_line(line: &str, width: usize) -> Vec<String> {
    if line.is_empty() {
        return vec![String::new()];
    }

    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;
    let mut ended_at_wrap_boundary = false;
    for ch in line.chars() {
        let char_width = ch.width().unwrap_or(0);
        if current_width > 0 && current_width.saturating_add(char_width) > width {
            lines.push(current);
            current = String::new();
            current_width = 0;
        }
        current.push(ch);
        current_width = current_width.saturating_add(char_width);
        if current_width >= width {
            lines.push(current);
            current = String::new();
            current_width = 0;
            ended_at_wrap_boundary = true;
        } else {
            ended_at_wrap_boundary = false;
        }
    }
    if !current.is_empty() {
        lines.push(current);
    } else if ended_at_wrap_boundary {
        lines.push(String::new());
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn wrapped_cursor_position(input_before_cursor: &str, content_width: usize) -> (usize, usize) {
    let width = content_width.max(1);
    let mut row = 0usize;
    let mut col = 0usize;

    for ch in input_before_cursor.chars() {
        if ch == '\n' {
            row = row.saturating_add(1);
            col = 0;
            continue;
        }
        let char_width = ch.width().unwrap_or(0);
        if col > 0 && col.saturating_add(char_width) > width {
            row = row.saturating_add(1);
            col = 0;
        }
        col = col.saturating_add(char_width);
        if col >= width {
            row = row.saturating_add(1);
            col = 0;
        }
    }

    (row, col)
}

fn clamp_char_boundary(input: &str, cursor_byte: usize) -> usize {
    let mut cursor_byte = cursor_byte.min(input.len());
    while cursor_byte > 0 && !input.is_char_boundary(cursor_byte) {
        cursor_byte -= 1;
    }
    cursor_byte
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct SegmentStyle(u16);

impl SegmentStyle {
    pub(super) const BOLD: u16 = 0b00_0000_0000_0001;
    pub(super) const ITALIC: u16 = 0b00_0000_0000_0010;
    pub(super) const UNDERLINED: u16 = 0b00_0000_0000_0100;
    pub(super) const DIM: u16 = 0b00_0000_0000_1000;
    pub(super) const CYAN: u16 = 0b00_0000_0001_0000;
    pub(super) const GREEN: u16 = 0b00_0000_0010_0000;
    pub(super) const BLUE: u16 = 0b00_0000_0100_0000;
    pub(super) const REVERSED: u16 = 0b00_0000_1000_0000;
    pub(super) const YELLOW: u16 = 0b00_0001_0000_0000;
    pub(super) const RED: u16 = 0b00_0010_0000_0000;
    pub(super) const MAGENTA: u16 = 0b00_0100_0000_0000;
    pub(super) const STATUS_BG: u16 = 0b00_1000_0000_0000;
    pub(super) const MODE_BG: u16 = 0b01_0000_0000_0000;

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

    pub(super) const fn warning() -> Self {
        Self(Self::YELLOW)
    }

    pub(super) const fn error() -> Self {
        Self(Self::RED)
    }

    pub(super) const fn status_bar() -> Self {
        Self(Self::STATUS_BG)
    }

    pub(super) const fn mode_badge() -> Self {
        Self(Self::MODE_BG)
    }

    pub(super) const fn status_warning() -> Self {
        Self(Self::STATUS_BG | Self::YELLOW)
    }

    pub(super) const fn merge(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub(super) const fn contains(self, flag: u16) -> bool {
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

#[allow(dead_code)]
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
    queue!(stdout, MoveTo(0, row), Clear(ClearType::CurrentLine)).map_err(terminal_error)?;
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
    if style.contains(SegmentStyle::REVERSED) {
        queue!(stdout, SetAttribute(Attribute::Reverse)).map_err(terminal_error)?;
    }
    if style.contains(SegmentStyle::MODE_BG) {
        queue!(
            stdout,
            SetForegroundColor(Color::AnsiValue(16)),
            SetBackgroundColor(Color::AnsiValue(42))
        )
        .map_err(terminal_error)?;
    } else if style.contains(SegmentStyle::STATUS_BG) {
        queue!(
            stdout,
            SetForegroundColor(Color::AnsiValue(231)),
            SetBackgroundColor(Color::AnsiValue(44))
        )
        .map_err(terminal_error)?;
    }
    if style.contains(SegmentStyle::RED) {
        queue!(stdout, SetForegroundColor(Color::Red)).map_err(terminal_error)?;
    } else if style.contains(SegmentStyle::YELLOW) {
        queue!(stdout, SetForegroundColor(Color::Yellow)).map_err(terminal_error)?;
    } else if style.contains(SegmentStyle::MAGENTA) {
        queue!(stdout, SetForegroundColor(Color::Magenta)).map_err(terminal_error)?;
    } else if style.contains(SegmentStyle::CYAN) {
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
    truncate_line_center(&compact, 80)
}

pub(super) fn truncate_line(line: &str, width: usize) -> String {
    take_prefix_width(line, width)
}

pub(super) fn truncate_line_center(line: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if visible_width(line) <= width {
        return line.to_string();
    }
    if width <= 1 {
        return "…".to_string();
    }
    let ellipsis_width = visible_width("…");
    let left_width = width.saturating_sub(ellipsis_width) / 2;
    let right_width = width.saturating_sub(ellipsis_width + left_width);
    let left = take_prefix_width(line, left_width);
    let right = take_suffix_width(line, right_width);
    format!("{left}…{right}")
}

pub(super) fn terminal_error(error: impl std::fmt::Display) -> CliError {
    CliError::Run(format!("terminal UI failed: {error}"))
}
