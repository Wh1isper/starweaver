use super::{
    pad_styled_line_with_style, push_detail_row, take_prefix_width, truncate_line, visible_width,
    with_codex_border, HitlPanelState, InteractiveTuiState, SegmentStyle, StyledLine,
    StyledSegment, TaskPanelItem,
};

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
        text: "Use `starweaver-cli approval list`, then approve or reject the pending approval id."
            .to_string(),
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

pub(super) fn render_task_panel(items: &[TaskPanelItem], width: usize) -> Vec<StyledLine> {
    if width < 4 {
        return Vec::new();
    }
    let inner_width = width.saturating_sub(4);
    let completed = items
        .iter()
        .filter(|item| item.status == "completed")
        .count();
    let in_progress = items
        .iter()
        .filter(|item| item.status.starts_with("in_progress"))
        .count();
    let mut rows = Vec::<Vec<StyledSegment>>::new();
    rows.push(vec![
        StyledSegment {
            text: "Tasks".to_string(),
            style: SegmentStyle::code().merge(SegmentStyle::bold()),
        },
        StyledSegment {
            text: format!(
                "  Progress: {completed}/{}{}",
                items.len(),
                if in_progress > 0 {
                    format!(" ({in_progress} in progress)")
                } else {
                    String::new()
                }
            ),
            style: SegmentStyle::dim(),
        },
    ]);
    for item in items.iter().take(12) {
        rows.push(render_task_row(item, inner_width));
    }
    if items.len() > 12 {
        rows.push(vec![StyledSegment {
            text: format!("  ... {} more task(s)", items.len() - 12),
            style: SegmentStyle::dim(),
        }]);
    }
    let mut lines = vec![StyledLine::plain("")];
    lines.extend(with_codex_border(rows, inner_width));
    lines
}

fn render_task_row(item: &TaskPanelItem, inner_width: usize) -> Vec<StyledSegment> {
    let id_width = 8usize.min(inner_width.saturating_sub(18).max(4));
    let status_label = truncate_line(&task_status_label(item), 11);
    let details = task_details_label(item);
    let subject_width = inner_width
        .saturating_sub(id_width)
        .saturating_sub(visible_width(&status_label))
        .saturating_sub(visible_width(&details))
        .saturating_sub(8)
        .max(1);
    let status_style = task_status_style(&item.status);
    let subject_style = if item.status == "completed" {
        SegmentStyle::dim()
    } else {
        SegmentStyle::default()
    };
    let mut row = vec![
        StyledSegment {
            text: "  #".to_string(),
            style: SegmentStyle::dim(),
        },
        StyledSegment {
            text: format!("{:<id_width$}", take_prefix_width(&item.id, id_width)),
            style: SegmentStyle::dim(),
        },
        StyledSegment {
            text: format!(" [{status_label:<11}] "),
            style: status_style,
        },
        StyledSegment {
            text: truncate_line(&item.subject, subject_width),
            style: subject_style,
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

fn task_status_label(item: &TaskPanelItem) -> String {
    if item.status == "in_progress" {
        item.active_form
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .map_or_else(
                || item.status.clone(),
                |active| format!("in_progress: {active}"),
            )
    } else {
        item.status.clone()
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
    vec![
        pad_styled_line_with_style(
            render_status_bar_primary(state, width),
            width,
            SegmentStyle::status_bar(),
        ),
        pad_styled_line_with_style(
            render_status_bar_secondary(state, width),
            width,
            SegmentStyle::status_bar(),
        ),
    ]
}

fn render_status_bar_primary(state: &InteractiveTuiState, width: usize) -> StyledLine {
    let mut line = StyledLine::styled(
        format!(" {} ", state.input_mode_label()),
        SegmentStyle::mode_badge().merge(SegmentStyle::bold()),
    );
    push_bounded_status_segment(
        &mut line,
        width,
        primary_state_text(state),
        status_style(state),
    );
    push_bounded_status_segment(
        &mut line,
        width,
        format!("Context: {}", state.context_percent_label()),
        SegmentStyle::status_bar(),
    );
    if let Some(transport) = state.model_transport_status.as_deref() {
        push_bounded_status_segment(
            &mut line,
            width,
            transport,
            SegmentStyle::status_warning().merge(SegmentStyle::bold()),
        );
    }
    if state.goal_active {
        push_bounded_status_segment(
            &mut line,
            width,
            format!(
                "Goal: {}/{}",
                state.goal_iteration, state.goal_max_iterations
            ),
            SegmentStyle::status_warning().merge(SegmentStyle::bold()),
        );
    }
    if let Some(activity) = state.active_tool_label() {
        push_bounded_status_segment(
            &mut line,
            width,
            activity,
            SegmentStyle::status_warning().merge(SegmentStyle::bold()),
        );
    }
    if state.pasted_image_count() > 0 {
        push_bounded_status_segment(
            &mut line,
            width,
            format!("images:{}", state.pasted_image_count()),
            SegmentStyle::status_warning(),
        );
    }
    push_optional_status_segment(
        &mut line,
        width,
        format!("Model: {}", state.model),
        SegmentStyle::status_bar(),
    );
    line
}

fn render_status_bar_secondary(state: &InteractiveTuiState, width: usize) -> StyledLine {
    let mut line = StyledLine::styled(
        secondary_status_text(state, width),
        SegmentStyle::status_bar(),
    );
    if !state.is_at_bottom() {
        push_status_segment(
            &mut line,
            format!("Scrolled: {}", state.scroll_offset),
            SegmentStyle::status_warning(),
        );
    }
    if !state.profile.is_empty() {
        push_optional_status_segment(
            &mut line,
            width,
            format!("Profile: {}", state.profile),
            SegmentStyle::status_bar(),
        );
    }
    if let Some(session) = state.session_id.as_deref() {
        push_optional_status_segment(
            &mut line,
            width,
            format!("Session: {session}"),
            SegmentStyle::status_bar(),
        );
    }
    line
}

fn push_status_segment(line: &mut StyledLine, text: impl Into<String>, style: SegmentStyle) {
    let text = text.into();
    if text.is_empty() {
        return;
    }
    line.push(" | ", SegmentStyle::status_bar());
    line.push(text, style.merge(SegmentStyle::status_bar()));
}

fn push_bounded_status_segment(
    line: &mut StyledLine,
    width: usize,
    text: impl AsRef<str>,
    style: SegmentStyle,
) {
    let separator_width = visible_width(" | ");
    let available = width.saturating_sub(line.visible_width());
    if available <= separator_width {
        return;
    }
    let content_width = available.saturating_sub(separator_width);
    if content_width == 0 {
        return;
    }
    push_status_segment(line, truncate_line(text.as_ref(), content_width), style);
}

fn push_optional_status_segment(
    line: &mut StyledLine,
    width: usize,
    text: impl AsRef<str>,
    style: SegmentStyle,
) {
    let separator_width = visible_width(" | ");
    let used = line.visible_width();
    let available = width.saturating_sub(used);
    if available <= separator_width {
        return;
    }
    let content_width = available.saturating_sub(separator_width);
    if content_width < 8 {
        return;
    }
    push_status_segment(line, truncate_line(text.as_ref(), content_width), style);
}

fn primary_state_text(state: &InteractiveTuiState) -> String {
    if state.running {
        phase_display(state)
    } else {
        format!("State: {}", state.status)
    }
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

fn pick_status_candidate(width: usize, candidates: &[String]) -> String {
    candidates
        .iter()
        .find(|candidate| visible_width(candidate) <= width)
        .cloned()
        .unwrap_or_else(|| candidates.last().cloned().unwrap_or_default())
}

fn secondary_status_text(state: &InteractiveTuiState, width: usize) -> String {
    if state.pending_hitl().is_some() {
        return pick_status_candidate(
            width,
            &[
                "Approval required: run `starweaver-cli approval list`, then approve or reject the pending approval | PageUp/PageDown/Mouse: Scroll".to_string(),
                "Approval required | approve/reject pending approval | PgUp/PgDn: Scroll".to_string(),
                "Approval required | PgUp/PgDn Scroll | Ctrl+C Interrupt".to_string(),
                "Approval required | Ctrl+C interrupt".to_string(),
            ],
        );
    }
    if state.selection_mode_visible() {
        return pick_status_candidate(
            width,
            &[
                "Mouse drag: Select terminal text to copy | Up/Down: Move marker | Enter/Esc: Close selection".to_string(),
                "Select text | Up/Down: Move | Enter/Esc: Close".to_string(),
                "Select | Enter/Esc Close".to_string(),
            ],
        );
    }
    if state.session_picker_visible() {
        pick_status_candidate(
            width,
            &[
                "Up/Down: Select session | Enter: Reload | Esc: Cancel | PageUp/PageDown/Mouse: Scroll".to_string(),
                "Up/Down: Select | Enter: Reload | Esc: Cancel | PgUp/PgDn: Scroll".to_string(),
                "↑/↓ Select | Enter | Esc | PgUp/PgDn Scroll".to_string(),
                "↑/↓ Select | Enter | Esc".to_string(),
            ],
        )
    } else if state.model_picker_visible() {
        pick_status_candidate(
            width,
            &[
                "Up/Down: Select model | Enter: Use | Esc: Cancel | PageUp/PageDown/Mouse: Scroll"
                    .to_string(),
                "Up/Down: Select | Enter: Use | Esc: Cancel | PgUp/PgDn: Scroll".to_string(),
                "↑/↓ Select | Enter | Esc | PgUp/PgDn Scroll".to_string(),
                "↑/↓ Select | Enter | Esc".to_string(),
            ],
        )
    } else if state.running {
        pick_status_candidate(
            width,
            &[
                format!(
                    "{} | {} | Ctrl+C: Interrupt | PageUp/PageDown/Mouse: Scroll",
                    state.enter_action_label(),
                    state.enter_toggle_label()
                ),
                format!(
                    "{} | {} | Ctrl+C: Interrupt | PgUp/PgDn: Scroll",
                    state.enter_action_label(),
                    state.enter_toggle_label()
                ),
                "Ctrl+C Interrupt | PgUp/PgDn Scroll".to_string(),
                "Ctrl+C Interrupt".to_string(),
            ],
        )
    } else if state.input.trim().is_empty() && state.pasted_image_count() == 0 {
        pick_status_candidate(
            width,
            &[
                format!(
                    "{} | {} | Ctrl+V: Attach clipboard image | Up/Down: History | Alt+Up/Down: Input scroll | PageUp/PageDown/Mouse: Scroll | Esc: Select | Ctrl+C: Exit",
                    state.enter_action_label(),
                    state.enter_toggle_label()
                ),
                format!(
                    "{} | {} | Ctrl+V: Image | ↑/↓: History | PgUp/PgDn: Scroll | Esc: Select | Ctrl+C: Exit",
                    state.enter_action_label(),
                    state.enter_toggle_label()
                ),
                "Enter Send | PgUp/PgDn Scroll | Esc Select | Ctrl+C Exit".to_string(),
                "Enter Send | Esc Select | Ctrl+C Exit".to_string(),
            ],
        )
    } else {
        pick_status_candidate(
            width,
            &[
                format!(
                    "{} | {} | Ctrl+V: Attach clipboard image | Up/Down: History | Alt+Up/Down: Input scroll | Ctrl+U: Clear | Esc: Select | Ctrl+C: Exit",
                    state.enter_action_label(),
                    state.enter_toggle_label()
                ),
                format!(
                    "{} | Ctrl+V: Image | ↑/↓: History | Alt+↑/↓: Input | Ctrl+U: Clear | Esc: Select | Ctrl+C: Exit",
                    state.enter_action_label()
                ),
                "Enter Send | Ctrl+U: Clear | Esc Select | Ctrl+C Exit".to_string(),
                "Enter Send | Ctrl+U Clear | Ctrl+C Exit".to_string(),
            ],
        )
    }
}
