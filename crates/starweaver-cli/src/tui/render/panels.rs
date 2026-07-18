use crate::tui::state::ClarifyingQuestionUiState;

use super::{
    HitlPanelState, InteractiveTuiState, SegmentStyle, StyledLine, StyledSegment, TaskPanelItem,
    pad_styled_line_with_style, push_detail_row, take_prefix_width, truncate_line, visible_width,
    with_codex_border,
};

pub(super) fn render_command_palette(state: &InteractiveTuiState, width: usize) -> Vec<StyledLine> {
    let Some(palette) = state.command_palette() else {
        return Vec::new();
    };
    if width < 4 {
        return Vec::new();
    }
    let inner_width = width.saturating_sub(4);
    let visible_count = 8usize.min(palette.items.len());
    let max_start = palette.items.len().saturating_sub(visible_count);
    let start = palette
        .selected
        .saturating_sub(visible_count / 2)
        .min(max_start);
    let end = start.saturating_add(visible_count).min(palette.items.len());
    let mut rows = vec![vec![
        StyledSegment {
            text: palette.title.to_ascii_uppercase(),
            style: SegmentStyle::code().merge(SegmentStyle::bold()),
        },
        StyledSegment {
            text: format!("  {} candidate(s)", palette.items.len()),
            style: SegmentStyle::dim(),
        },
    ]];
    if start > 0 {
        rows.push(vec![StyledSegment {
            text: format!("  … {start} earlier"),
            style: SegmentStyle::dim(),
        }]);
    }
    for (index, item) in palette.items[start..end].iter().enumerate() {
        let absolute = start + index;
        let selected = absolute == palette.selected;
        rows.push(vec![
            StyledSegment {
                text: if selected { "> " } else { "  " }.to_string(),
                style: if selected {
                    SegmentStyle::warning().merge(SegmentStyle::bold())
                } else {
                    SegmentStyle::dim()
                },
            },
            StyledSegment {
                text: item.label.clone(),
                style: if selected {
                    SegmentStyle::bold()
                } else {
                    SegmentStyle::default()
                },
            },
            StyledSegment {
                text: format!("  [{}]", item.source.label()),
                style: SegmentStyle::dim(),
            },
        ]);
        if selected {
            push_detail_row(
                &mut rows,
                "",
                &item.detail,
                inner_width,
                SegmentStyle::dim(),
            );
        }
    }
    if end < palette.items.len() {
        rows.push(vec![StyledSegment {
            text: format!("  … {} more", palette.items.len() - end),
            style: SegmentStyle::dim(),
        }]);
    }
    rows.push(Vec::new());
    rows.push(vec![StyledSegment {
        text: "↑/↓: Move · Tab: Complete · Shift-Tab: Previous · Enter: Run · Esc: Close"
            .to_string(),
        style: SegmentStyle::dim(),
    }]);
    let mut lines = vec![StyledLine::plain("")];
    lines.extend(with_codex_border(rows, inner_width));
    lines
}

pub(super) fn render_selection_panel(state: &InteractiveTuiState, width: usize) -> Vec<StyledLine> {
    let style = SegmentStyle::status_warning().merge(SegmentStyle::bold());
    let position = state
        .selection_position_label()
        .unwrap_or_else(|| "0/0".to_string());
    let preview = state
        .selected_line_preview()
        .unwrap_or_else(|| "No transcript lines available".to_string());
    let mut line = StyledLine::styled(
        " SELECT ",
        SegmentStyle::mode_badge().merge(SegmentStyle::bold()),
    );
    line.push(" ", SegmentStyle::status_bar());
    line.push(position, style);
    line.push(" | ", SegmentStyle::status_bar());
    line.push(preview, SegmentStyle::status_bar());
    line.push(
        " | Mouse drag: copy text | Up/Down: Move | Enter/Esc: Close",
        SegmentStyle::status_bar(),
    );
    vec![pad_styled_line_with_style(
        line,
        width,
        SegmentStyle::status_bar(),
    )]
}

pub(super) fn render_hitl_panel(hitl: &HitlPanelState, width: usize) -> Vec<StyledLine> {
    if let Some(clarifying) = hitl.clarifying.as_ref() {
        return render_clarifying_question_panel(hitl, clarifying, width);
    }
    if width < 4 {
        return Vec::new();
    }
    let inner_width = width.saturating_sub(4);
    let mut rows = Vec::<Vec<StyledSegment>>::new();
    rows.push(vec![
        StyledSegment {
            text: "Tool Approval Required".to_string(),
            style: SegmentStyle::warning().merge(SegmentStyle::bold()),
        },
        StyledSegment {
            text: "  Review the pending shell/tool action before continuing".to_string(),
            style: SegmentStyle::dim(),
        },
    ]);
    if let Some(approval_id) = hitl.approval_id.as_deref() {
        push_detail_row(
            &mut rows,
            "approval:",
            approval_id,
            inner_width,
            SegmentStyle::dim(),
        );
    }
    push_detail_row(
        &mut rows,
        "tool:",
        &hitl.tool_name,
        inner_width,
        SegmentStyle::default(),
    );
    push_detail_row(
        &mut rows,
        "tool_call:",
        &hitl.tool_call_id,
        inner_width,
        SegmentStyle::dim(),
    );
    if let Some(command) = hitl.command.as_deref() {
        push_detail_row(
            &mut rows,
            "command:",
            command,
            inner_width,
            SegmentStyle::warning(),
        );
    }
    if let Some(request) = hitl.request_preview.as_deref() {
        push_detail_row(
            &mut rows,
            "request:",
            request,
            inner_width,
            SegmentStyle::code(),
        );
    }
    if let Some(risk) = hitl.risk_level.as_deref() {
        push_detail_row(&mut rows, "risk:", risk, inner_width, hitl_risk_style(risk));
    }
    if let Some(reason) = hitl.reason.as_deref() {
        push_detail_row(
            &mut rows,
            "reason:",
            reason,
            inner_width,
            SegmentStyle::default(),
        );
    }
    rows.push(Vec::new());
    rows.push(vec![StyledSegment {
        text: if hitl.approval_id.is_some() {
            "[a/y] Approve    [r/n] Reject    [Esc] Refresh".to_string()
        } else {
            "Persisting approval request…    [Esc] Refresh".to_string()
        },
        style: SegmentStyle::dim(),
    }]);
    let mut lines = vec![StyledLine::plain("")];
    lines.extend(with_codex_border(rows, inner_width));
    lines
}

#[allow(clippy::too_many_lines)]
fn render_clarifying_question_panel(
    hitl: &HitlPanelState,
    clarifying: &ClarifyingQuestionUiState,
    width: usize,
) -> Vec<StyledLine> {
    if width < 4 {
        return Vec::new();
    }
    let inner_width = width.saturating_sub(4);
    let Some(question) = clarifying.current_question() else {
        return Vec::new();
    };
    let mode = if question.multi_select {
        "Multiple choice"
    } else {
        "Single choice"
    };
    let mut rows = vec![vec![
        StyledSegment {
            text: format!(
                "Question {}/{} · {}",
                clarifying.question_index + 1,
                clarifying.questions.len(),
                question.header
            ),
            style: SegmentStyle::warning().merge(SegmentStyle::bold()),
        },
        StyledSegment {
            text: format!("  {mode}"),
            style: SegmentStyle::dim(),
        },
    ]];
    push_detail_row(
        &mut rows,
        "question:",
        &question.question,
        inner_width,
        SegmentStyle::default().merge(SegmentStyle::bold()),
    );
    let selected = clarifying
        .selections
        .get(clarifying.question_index)
        .cloned()
        .unwrap_or_default();
    for (index, option) in question.options.iter().enumerate() {
        let focused = !clarifying.free_form_active && index == clarifying.option_index;
        let checked = selected.contains(&index);
        let marker = if question.multi_select {
            if checked { "[x]" } else { "[ ]" }
        } else if checked {
            "(*)"
        } else {
            "( )"
        };
        rows.push(vec![
            StyledSegment {
                text: if focused { "> " } else { "  " }.to_string(),
                style: if focused {
                    SegmentStyle::warning().merge(SegmentStyle::bold())
                } else {
                    SegmentStyle::dim()
                },
            },
            StyledSegment {
                text: format!("{marker} {}", option.label),
                style: if focused {
                    SegmentStyle::bold()
                } else {
                    SegmentStyle::default()
                },
            },
        ]);
        if focused {
            push_detail_row(
                &mut rows,
                "",
                &option.description,
                inner_width,
                SegmentStyle::dim(),
            );
            if let Some(preview) = option.preview.as_deref() {
                push_detail_row(
                    &mut rows,
                    "preview:",
                    preview,
                    inner_width,
                    SegmentStyle::code(),
                );
            }
        }
    }
    if let Some(answer) = clarifying.free_form_answers.get(&clarifying.question_index) {
        push_detail_row(
            &mut rows,
            "custom:",
            answer,
            inner_width,
            SegmentStyle::default(),
        );
    }
    rows.push(Vec::new());
    rows.push(vec![StyledSegment {
        text: if hitl.approval_id.is_none() {
            "Persisting question request…    Esc: Refresh".to_string()
        } else if clarifying.free_form_active {
            "Type a custom answer below · Enter: Confirm · Esc: Choices".to_string()
        } else if question.multi_select {
            "↑/↓: Move · Space: Toggle · Enter: Continue · E: Custom · Tab: Next question"
                .to_string()
        } else {
            "↑/↓: Select · Enter: Continue · E: Custom · Tab: Next question".to_string()
        },
        style: SegmentStyle::dim(),
    }]);
    let mut lines = vec![StyledLine::plain("")];
    lines.extend(with_codex_border(rows, inner_width));
    lines
}

fn hitl_risk_style(risk: &str) -> SegmentStyle {
    match risk {
        "high" | "extra_high" | "extra-high" => SegmentStyle::error().merge(SegmentStyle::bold()),
        "medium" => SegmentStyle::warning().merge(SegmentStyle::bold()),
        _ => SegmentStyle::default(),
    }
}

pub(super) fn render_task_summary(state: &InteractiveTuiState, width: usize) -> Vec<StyledLine> {
    let items = state.task_panel_items();
    if items.is_empty() || width == 0 {
        return Vec::new();
    }
    let completed = items
        .iter()
        .filter(|item| item.status == "completed")
        .count();
    let blocked = items
        .iter()
        .filter(|item| !item.blocked_by.is_empty() || item.status == "blocked")
        .count();
    let current = items
        .iter()
        .find(|item| item.status.starts_with("in_progress"))
        .or_else(|| items.iter().find(|item| item.status != "completed"));
    let mut text = format!("Tasks {completed}/{}", items.len());
    if let Some(current) = current {
        let active = current
            .active_form
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(&current.subject);
        text.push_str(" · Current: #");
        text.push_str(&current.id);
        text.push(' ');
        text.push_str(active);
    }
    if blocked > 0 {
        text.push_str(" · ");
        text.push_str(&blocked.to_string());
        text.push_str(" blocked");
    }
    vec![pad_styled_line_with_style(
        StyledLine::styled(
            truncate_line(&text, width),
            SegmentStyle::status_bar().merge(SegmentStyle::bold()),
        ),
        width,
        SegmentStyle::status_bar(),
    )]
}

#[allow(clippy::too_many_lines)]
pub(super) fn render_task_panel(state: &InteractiveTuiState, width: usize) -> Vec<StyledLine> {
    let items = state.task_panel_items();
    if width < 4 || items.is_empty() {
        return Vec::new();
    }
    let inner_width = width.saturating_sub(4);
    let completed = items
        .iter()
        .filter(|item| item.status == "completed")
        .count();
    let mut rows = vec![vec![
        StyledSegment {
            text: "Tasks".to_string(),
            style: SegmentStyle::code().merge(SegmentStyle::bold()),
        },
        StyledSegment {
            text: format!("  {completed}/{} complete", items.len()),
            style: SegmentStyle::dim(),
        },
    ]];
    let anchor = state.task_panel_index().min(items.len().saturating_sub(1));
    let visible_count = 8usize.min(items.len());
    let start = anchor
        .saturating_sub(visible_count / 2)
        .min(items.len().saturating_sub(visible_count));
    let end = start.saturating_add(visible_count).min(items.len());
    if start > 0 {
        rows.push(vec![StyledSegment {
            text: format!("  … {start} earlier"),
            style: SegmentStyle::dim(),
        }]);
    }
    for item in &items[start..end] {
        rows.push(render_task_row(item, false, inner_width));
    }
    if end < items.len() {
        rows.push(vec![StyledSegment {
            text: format!("  … {} more", items.len() - end),
            style: SegmentStyle::dim(),
        }]);
    }
    rows.push(Vec::new());
    rows.push(vec![StyledSegment {
        text: "F2: Close".to_string(),
        style: SegmentStyle::dim(),
    }]);
    let mut lines = vec![StyledLine::plain("")];
    lines.extend(with_codex_border(rows, inner_width));
    lines
}

fn render_task_row(item: &TaskPanelItem, selected: bool, inner_width: usize) -> Vec<StyledSegment> {
    let marker = task_status_marker(item);
    let details = task_details_label(item);
    let active = item
        .active_form
        .as_deref()
        .filter(|value| item.status.starts_with("in_progress") && !value.trim().is_empty())
        .unwrap_or(&item.subject);
    let prefix_width = visible_width(&format!("  #{} {marker} ", item.id));
    let details_width = visible_width(&details);
    let subject_width = inner_width
        .saturating_sub(prefix_width)
        .saturating_sub(details_width)
        .saturating_sub(usize::from(!details.is_empty()))
        .max(1);
    let mut row = vec![
        StyledSegment {
            text: if selected { "> " } else { "  " }.to_string(),
            style: if selected {
                SegmentStyle::warning().merge(SegmentStyle::bold())
            } else {
                SegmentStyle::dim()
            },
        },
        StyledSegment {
            text: format!("#{} ", take_prefix_width(&item.id, 8)),
            style: SegmentStyle::dim(),
        },
        StyledSegment {
            text: format!("{marker} "),
            style: task_status_style(&item.status),
        },
        StyledSegment {
            text: truncate_line(active, subject_width),
            style: if selected {
                SegmentStyle::bold()
            } else if item.status == "completed" {
                SegmentStyle::dim()
            } else {
                SegmentStyle::default()
            },
        },
    ];
    if !details.is_empty() {
        row.push(StyledSegment {
            text: format!(" {details}"),
            style: SegmentStyle::dim(),
        });
    }
    row
}

fn task_status_marker(item: &TaskPanelItem) -> &'static str {
    if item.status == "completed" {
        "[x]"
    } else if item.status.starts_with("in_progress") {
        "[>]"
    } else if item.status == "blocked" || !item.blocked_by.is_empty() {
        "[!]"
    } else {
        "[ ]"
    }
}

fn task_details_label(item: &TaskPanelItem) -> String {
    let mut details = Vec::new();
    if let Some(owner) = item
        .owner
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        details.push(format!("@{owner}"));
    }
    if !item.blocked_by.is_empty() {
        details.push(format!("[blocked by #{}]", item.blocked_by.join(", #")));
    }
    if !item.blocks.is_empty() {
        details.push(format!("[blocks #{}]", item.blocks.join(", #")));
    }
    details.join(" ")
}

fn task_status_style(status: &str) -> SegmentStyle {
    match status {
        "completed" => SegmentStyle::blockquote().merge(SegmentStyle::bold()),
        status if status.starts_with("in_progress") => {
            SegmentStyle::code().merge(SegmentStyle::bold())
        }
        _ => SegmentStyle::default(),
    }
}

pub(super) fn render_status_bar_lines(
    state: &InteractiveTuiState,
    width: usize,
) -> Vec<StyledLine> {
    if width == 0 {
        return Vec::new();
    }
    render_semantic_status_bar(state, width)
        .into_iter()
        .map(|line| pad_styled_line_with_style(line, width, SegmentStyle::status_bar()))
        .collect()
}

fn render_semantic_status_bar(state: &InteractiveTuiState, width: usize) -> Vec<StyledLine> {
    let (badge, badge_style) = status_badge(state);
    let badge = format!(" {badge} ");
    let mut lines = vec![StyledLine::styled(
        truncate_line(&badge, width),
        SegmentStyle::mode_badge().merge(badge_style),
    )];
    let remaining = width.saturating_sub(lines[0].visible_width());
    let action =
        pick_status_candidate(remaining.saturating_sub(3), status_action_candidates(state));
    if !action.is_empty() {
        push_bounded_status_segment(&mut lines[0], width, action, badge_style);
    }
    if let Some(activity) = status_activity(state) {
        push_wrapped_status_segment(
            &mut lines,
            width,
            activity,
            SegmentStyle::status_warning().merge(SegmentStyle::bold()),
        );
    }
    if state.pasted_image_count() > 0 {
        push_wrapped_status_segment(
            &mut lines,
            width,
            format!("images {}", state.pasted_image_count()),
            SegmentStyle::status_warning(),
        );
    }
    if !state.is_at_bottom() {
        push_wrapped_status_segment(
            &mut lines,
            width,
            format!("{} new", state.unread_output_lines),
            SegmentStyle::status_warning(),
        );
    }
    push_wrapped_status_segment(
        &mut lines,
        width,
        format!("cost {}", state.session_cost_label()),
        SegmentStyle::status_bar(),
    );
    push_wrapped_status_segment(
        &mut lines,
        width,
        format!("time {}", state.session_elapsed_label()),
        SegmentStyle::status_bar(),
    );
    push_wrapped_status_segment(
        &mut lines,
        width,
        format!("ctx {}", state.context_percent_label()),
        SegmentStyle::status_bar(),
    );
    if let Some(notification) = state.input_notification() {
        push_wrapped_status_segment(
            &mut lines,
            width,
            notification,
            SegmentStyle::status_warning(),
        );
    }
    if width >= 100 && !state.profile.is_empty() {
        push_wrapped_status_segment(
            &mut lines,
            width,
            &state.profile,
            SegmentStyle::status_bar(),
        );
    }
    if width >= 120 {
        push_wrapped_status_segment(&mut lines, width, &state.model, SegmentStyle::status_bar());
    }
    lines
}

fn status_badge(state: &InteractiveTuiState) -> (&'static str, SegmentStyle) {
    if state.clarifying_answer_ready() {
        (
            "QUESTION",
            SegmentStyle::warning().merge(SegmentStyle::bold()),
        )
    } else if state.pending_hitl().is_some() || state.hitl_reload_session_id().is_some() {
        (
            "APPROVAL",
            SegmentStyle::warning().merge(SegmentStyle::bold()),
        )
    } else if state.command_palette_visible() {
        ("COMMAND", SegmentStyle::code().merge(SegmentStyle::bold()))
    } else if state.history_search_visible() {
        ("HISTORY", SegmentStyle::code().merge(SegmentStyle::bold()))
    } else if state.help_panel_visible() {
        ("HELP", SegmentStyle::code().merge(SegmentStyle::bold()))
    } else if state.session_picker_visible() {
        ("SESSION", SegmentStyle::code().merge(SegmentStyle::bold()))
    } else if state.model_picker_visible() {
        ("MODEL", SegmentStyle::code().merge(SegmentStyle::bold()))
    } else if state.selection_mode_visible() {
        (
            "SELECT",
            SegmentStyle::warning().merge(SegmentStyle::bold()),
        )
    } else if state.status == "ERROR" {
        ("ERROR", SegmentStyle::error().merge(SegmentStyle::bold()))
    } else if state.status == "WAITING" {
        (
            "WAITING",
            SegmentStyle::warning().merge(SegmentStyle::bold()),
        )
    } else if !state.is_at_bottom() {
        (
            "PAUSED",
            SegmentStyle::warning().merge(SegmentStyle::bold()),
        )
    } else if state.running || state.shell_running() {
        ("RUNNING", SegmentStyle::code().merge(SegmentStyle::bold()))
    } else {
        ("READY", SegmentStyle::bold())
    }
}

fn status_action_candidates(state: &InteractiveTuiState) -> &'static [&'static str] {
    if state.clarifying_answer_ready() {
        if state.clarifying_free_form_active() {
            &["Enter confirm · Esc choices", "Enter confirm", "Enter"]
        } else if state
            .clarifying_question()
            .and_then(ClarifyingQuestionUiState::current_question)
            .is_some_and(|question| question.multi_select)
        {
            &[
                "↑/↓ move · Space toggle · Enter next",
                "Space toggle · Enter next",
                "Enter next",
                "Enter",
            ]
        } else {
            &[
                "↑/↓ select · Enter next",
                "Enter next · E custom",
                "Enter next",
                "Enter",
            ]
        }
    } else if state.pending_hitl().is_some() || state.hitl_reload_session_id().is_some() {
        if state.hitl_decision_ready() {
            &["A approve · R reject", "A approve · R reject", "A/R"]
        } else {
            &["Esc refresh", "Esc"]
        }
    } else if state.command_palette_visible() {
        &[
            "↑/↓ move · Tab complete · Enter run",
            "Tab complete · Enter run",
            "Tab · Enter",
            "Enter",
        ]
    } else if state.history_search_visible() {
        &[
            "Ctrl+R older · Enter use · Esc",
            "Enter use · Esc",
            "Enter · Esc",
        ]
    } else if state.help_panel_visible() {
        &["Esc close", "Esc"]
    } else if state.session_picker_visible() {
        &[
            "↑/↓ select · Enter reload · Esc",
            "Enter reload · Esc",
            "Enter · Esc",
        ]
    } else if state.model_picker_visible() {
        &[
            "↑/↓ select · Enter use · Esc",
            "Enter use · Esc",
            "Enter · Esc",
        ]
    } else if state.selection_mode_visible() {
        &["↑/↓ move · Enter/Esc close", "Enter/Esc close", "Esc"]
    } else if !state.is_at_bottom() {
        &[
            "Ctrl+L follow · PgUp/PgDn scroll",
            "Ctrl+L follow",
            "Ctrl+L",
        ]
    } else if state.running || state.shell_running() {
        &[
            "Ctrl+C interrupt · Enter steer",
            "Ctrl+C interrupt",
            "Ctrl+C",
        ]
    } else if state.composer_has_draft() {
        &[
            "Enter send · Ctrl+O newline · Ctrl+U clear",
            "Enter send · Ctrl+O newline",
            "Enter send",
        ]
    } else {
        &[
            "Enter send · / commands · ? help",
            "Enter send · / commands",
            "Enter send",
            "Enter",
        ]
    }
}

fn status_activity(state: &InteractiveTuiState) -> Option<String> {
    if state.clarifying_answer_ready() {
        return state.clarifying_question().map(|clarifying| {
            format!(
                "{}/{}",
                clarifying.question_index + 1,
                clarifying.questions.len()
            )
        });
    }
    if state.running {
        if let Some(transport) = state.model_transport_status.as_ref() {
            return Some(transport.clone());
        }
        if let Some(activity) = state.active_tool_label() {
            return Some(activity);
        }
        let phase = state.phase.as_str();
        return (!phase.is_empty()).then(|| phase.to_string());
    }
    if state.goal_active {
        return Some(format!(
            "goal {}/{}",
            state.goal_iteration, state.goal_max_iterations
        ));
    }
    state.model_transport_status.clone()
}

fn pick_status_candidate(width: usize, candidates: &[&str]) -> String {
    candidates
        .iter()
        .find(|candidate| visible_width(candidate) <= width)
        .copied()
        .unwrap_or_else(|| candidates.last().copied().unwrap_or_default())
        .to_string()
}

fn push_status_segment(line: &mut StyledLine, text: impl Into<String>, style: SegmentStyle) {
    let text = text.into();
    if text.is_empty() {
        return;
    }
    line.push(" · ", SegmentStyle::status_bar());
    line.push(text, style.merge(SegmentStyle::status_bar()));
}

fn push_bounded_status_segment(
    line: &mut StyledLine,
    width: usize,
    text: impl AsRef<str>,
    style: SegmentStyle,
) {
    let separator_width = visible_width(" · ");
    let available = width.saturating_sub(line.visible_width());
    if available <= separator_width {
        return;
    }
    let content_width = available.saturating_sub(separator_width);
    push_status_segment(line, truncate_line(text.as_ref(), content_width), style);
}

fn push_wrapped_status_segment(
    lines: &mut Vec<StyledLine>,
    width: usize,
    text: impl AsRef<str>,
    style: SegmentStyle,
) {
    let text = text.as_ref();
    if text.is_empty() || width == 0 {
        return;
    }
    let separator_width = visible_width(" · ");
    let fits_current = lines.last().is_some_and(|line| {
        line.visible_width()
            .saturating_add(separator_width)
            .saturating_add(visible_width(text))
            <= width
    });
    if fits_current {
        if let Some(line) = lines.last_mut() {
            push_status_segment(line, text, style);
        }
        return;
    }
    let mut line = StyledLine::styled(" ", SegmentStyle::status_bar());
    line.push(
        truncate_line(text, width.saturating_sub(1)),
        style.merge(SegmentStyle::status_bar()),
    );
    lines.push(line);
}
