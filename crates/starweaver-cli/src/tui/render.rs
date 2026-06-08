use std::{env, io};

use crossterm::{
    cursor::MoveTo,
    queue,
    style::{
        Attribute, Color, Print, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor,
    },
};
use serde_json::Value;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{CliError, CliResult};

use super::{
    markdown::{render_transcript_lines, ASSISTANT_CONTENT_PREFIX},
    snapshot::TuiSnapshot,
    state::{InteractiveTuiState, SteeringStatus},
};

const SESSION_HEADER_MAX_INNER_WIDTH: usize = 56;
const STARWEAVER_CLI_VERSION: &str = env!("CARGO_PKG_VERSION");

pub(super) fn snapshot_interactive_lines(snapshot: &TuiSnapshot) -> Vec<String> {
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
        startup_help_line("Tab", " - submit, or queue a draft while running"),
        startup_help_line("Ctrl-O", " - insert a newline"),
        startup_help_line("/help", " - print available commands"),
        startup_help_line("!<cmd>", " - execute a shell command"),
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
    let image_labels = state.pasted_image_labels();
    let mut lines = Vec::with_capacity(
        input_lines
            .len()
            .saturating_add(image_labels.len())
            .saturating_add(2),
    );
    lines.push(StyledLine::plain(""));
    for image in image_labels {
        let mut line =
            StyledLine::styled("  image ", SegmentStyle::dim().merge(SegmentStyle::code()));
        line.push(image, SegmentStyle::dim());
        lines.push(pad_styled_line(line, width));
    }
    for (offset, input) in input_lines.iter().enumerate() {
        let mut line = StyledLine::styled(
            if offset == 0 {
                composer_prompt(state)
            } else {
                " "
            },
            SegmentStyle::bold(),
        );
        line.push(" ", SegmentStyle::default());
        if input.is_empty() && offset == 0 {
            line.push(composer_placeholder(state), SegmentStyle::dim());
        } else {
            line.push(input, SegmentStyle::default());
        }
        lines.push(pad_styled_line(line, width));
    }
    lines.push(StyledLine::plain(""));
    lines
}

const fn composer_prompt(state: &InteractiveTuiState) -> &'static str {
    if state.running {
        "[scroll] *"
    } else {
        "[scroll] >"
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
    lines.extend(render_steering_lines(state, width));
    lines.extend(render_status_bar_lines(state, width));
    lines
}

fn render_steering_lines(state: &InteractiveTuiState, width: usize) -> Vec<StyledLine> {
    let style = SegmentStyle::steering_bar();
    if state.steering_items().is_empty() {
        return vec![pad_styled_line_with_style(
            StyledLine::styled(
                " [Steering messages will appear here during agent execution]",
                style,
            ),
            width,
            style,
        )];
    }
    state
        .steering_items()
        .iter()
        .rev()
        .map(|item| {
            let prefix = match item.status {
                SteeringStatus::Acked => "[v] ",
                SteeringStatus::Pending => ">>> ",
            };
            pad_styled_line_with_style(
                StyledLine::styled(format!("{prefix}{}", item.text), style),
                width,
                style,
            )
        })
        .collect()
}

fn render_status_bar_lines(state: &InteractiveTuiState, width: usize) -> Vec<StyledLine> {
    vec![
        pad_styled_line_with_style(
            render_status_bar_primary(state),
            width,
            SegmentStyle::status_bar(),
        ),
        pad_styled_line_with_style(
            render_status_bar_secondary(state),
            width,
            SegmentStyle::status_bar(),
        ),
    ]
}

fn render_status_bar_primary(state: &InteractiveTuiState) -> StyledLine {
    let mut line = StyledLine::styled(
        format!(" {} ", state.input_mode_label()),
        SegmentStyle::mode_badge().merge(SegmentStyle::bold()),
    );
    line.push(" | ", SegmentStyle::status_bar());
    if state.running {
        line.push(
            phase_display(state),
            status_style(state).merge(SegmentStyle::status_bar()),
        );
    } else {
        line.push(
            format!("State: {}", state.status),
            status_style(state).merge(SegmentStyle::status_bar()),
        );
    }
    line.push(" | ", SegmentStyle::status_bar());
    line.push(
        format!("Model: {}", state.model),
        SegmentStyle::status_bar(),
    );
    line.push(" | ", SegmentStyle::status_bar());
    line.push(
        format!("Context: {}", state.context_percent_label()),
        SegmentStyle::status_bar(),
    );
    if state.goal_active {
        line.push(" | ", SegmentStyle::status_bar());
        line.push(
            format!(
                "Goal: {}/{}",
                state.goal_iteration, state.goal_max_iterations
            ),
            SegmentStyle::status_warning().merge(SegmentStyle::bold()),
        );
    }
    if state.pasted_image_count() > 0 {
        line.push(" | ", SegmentStyle::status_bar());
        line.push(
            format!("images:{}", state.pasted_image_count()),
            SegmentStyle::status_warning(),
        );
    }
    line
}

fn render_status_bar_secondary(state: &InteractiveTuiState) -> StyledLine {
    let mut line = StyledLine::styled(secondary_status_text(state), SegmentStyle::status_bar());
    if !state.is_at_bottom() {
        line.push(" | ", SegmentStyle::status_bar());
        line.push(
            format!("Scrolled: {}", state.scroll_offset),
            SegmentStyle::status_warning(),
        );
    }
    if !state.profile.is_empty() {
        line.push(" | ", SegmentStyle::status_bar());
        line.push(
            format!("Profile: {}", state.profile),
            SegmentStyle::status_bar(),
        );
    }
    if let Some(session) = state.session_id.as_deref() {
        line.push(" | ", SegmentStyle::status_bar());
        line.push(format!("Session: {session}"), SegmentStyle::status_bar());
    }
    line
}

fn phase_display(state: &InteractiveTuiState) -> String {
    match state.phase.as_str() {
        "thinking" => "Thinking...".to_string(),
        "tools" => "Running tools...".to_string(),
        "streaming" => "Running...".to_string(),
        phase => phase.to_string(),
    }
}

fn status_style(state: &InteractiveTuiState) -> SegmentStyle {
    match state.status.as_str() {
        "ERROR" => SegmentStyle::error().merge(SegmentStyle::bold()),
        "WAITING" | "INTERRUPT" => SegmentStyle::warning().merge(SegmentStyle::bold()),
        _ => SegmentStyle::status_bar(),
    }
}

fn secondary_status_text(state: &InteractiveTuiState) -> &'static str {
    if state.running {
        "Ctrl+C: Interrupt"
    } else if state.input.trim().is_empty() && state.pasted_image_count() == 0 {
        "Enter:Send | Tab:Multiline | Ctrl+Up/Down: Scroll | Ctrl+C: Exit"
    } else {
        "Enter:Send | Tab:Multiline | Ctrl+U: Clear | Ctrl+C: Exit"
    }
}

#[cfg(test)]
pub(super) fn render_shortcut_overlay(width: usize) -> Vec<StyledLine> {
    render_help_panel(width)
}

pub(super) fn render_help_panel(width: usize) -> Vec<StyledLine> {
    let command_rows = [
        ("/help", "Print this help in the transcript"),
        ("/clear", "Clear output"),
        ("/cost", "Show usage and cost summary"),
        (
            "/goal <task>",
            "Run task toward a verified goal until complete",
        ),
    ];
    let key_rows = [
        ("Ctrl+C", "Interrupt active run or exit"),
        ("Ctrl+D", "Exit"),
        ("Ctrl+V", "Paste text or attach image paths"),
        ("Tab", "Send or queue a draft while running"),
        ("Ctrl+O", "Insert newline"),
        ("Up/Down, Ctrl+P/N", "Browse history"),
        ("PageUp/PageDown", "Scroll output"),
    ];
    let mut lines = Vec::new();
    lines.push(StyledLine::plain(""));
    lines.push(StyledLine::styled(
        "Available Commands",
        SegmentStyle::code().merge(SegmentStyle::bold()),
    ));
    for (command, description) in command_rows {
        lines.push(render_help_table_row(
            command,
            description,
            SegmentStyle::blockquote(),
            width,
        ));
    }
    lines.push(StyledLine::plain(""));
    lines.push(StyledLine::styled(
        "Shell",
        SegmentStyle::code().merge(SegmentStyle::bold()),
    ));
    lines.push(render_help_table_row(
        "!<cmd>",
        "Execute shell command directly",
        SegmentStyle::warning(),
        width,
    ));
    lines.push(StyledLine::plain(""));
    lines.push(StyledLine::styled(
        "Key Bindings",
        SegmentStyle::code().merge(SegmentStyle::bold()),
    ));
    for (key, description) in key_rows {
        lines.push(render_help_table_row(
            key,
            description,
            SegmentStyle::warning(),
            width,
        ));
    }
    lines
}

fn render_help_table_row(
    label: &str,
    description: &str,
    label_style: SegmentStyle,
    width: usize,
) -> StyledLine {
    let mut line = StyledLine::plain("  ");
    line.push(label, label_style);
    let label_width = 20usize;
    let used = visible_width(label).saturating_add(2);
    line.push(
        " ".repeat(label_width.saturating_sub(used).max(2)),
        SegmentStyle::dim(),
    );
    line.push(description, SegmentStyle::default());
    pad_styled_line(line, width)
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
    "[scroll] > ".len().saturating_add(visible_width(current))
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
    pub(super) const STEERING_BG: u16 = 0b10_0000_0000_0000;

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

    pub(super) const fn steering_bar() -> Self {
        Self(Self::STEERING_BG)
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
    } else if style.contains(SegmentStyle::STEERING_BG) {
        queue!(
            stdout,
            SetForegroundColor(Color::White),
            SetBackgroundColor(Color::AnsiValue(100))
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
    truncate_line(&compact, 80)
}

pub(super) fn truncate_line(line: &str, width: usize) -> String {
    take_prefix_width(line, width)
}

pub(super) fn terminal_error(error: impl std::fmt::Display) -> CliError {
    CliError::Run(format!("terminal UI failed: {error}"))
}
