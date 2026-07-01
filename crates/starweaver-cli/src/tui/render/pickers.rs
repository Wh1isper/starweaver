use super::{
    InteractiveTuiState, SegmentStyle, StyledLine, StyledSegment, compact_timestamp,
    take_prefix_width, visible_width, with_codex_border, wrap_text_width,
};
use crate::tui::{ModelChoice, SessionChoice};

pub(super) fn render_session_picker_panel(
    state: &InteractiveTuiState,
    width: usize,
) -> Vec<StyledLine> {
    if width < 4 {
        return Vec::new();
    }
    let inner_width = width.saturating_sub(4);
    let mut rows = Vec::<Vec<StyledSegment>>::new();
    rows.push(vec![
        StyledSegment {
            text: "Sessions".to_string(),
            style: SegmentStyle::code().merge(SegmentStyle::bold()),
        },
        StyledSegment {
            text: "  Enter: reload | Esc: close | Up/Down: navigate".to_string(),
            style: SegmentStyle::dim(),
        },
    ]);
    if state.session_choices().is_empty() {
        rows.push(vec![StyledSegment {
            text: "No sessions found.".to_string(),
            style: SegmentStyle::dim(),
        }]);
    } else {
        let max_visible = 8usize;
        let total = state.session_choices().len();
        let selected_index = state.session_picker_index().min(total.saturating_sub(1));
        let start = selected_index
            .saturating_sub(max_visible / 2)
            .min(total.saturating_sub(max_visible));
        let end = total.min(start.saturating_add(max_visible));
        if start > 0 {
            rows.push(vec![StyledSegment {
                text: "    ...".to_string(),
                style: SegmentStyle::dim(),
            }]);
        }
        for (index, choice) in state
            .session_choices()
            .iter()
            .enumerate()
            .skip(start)
            .take(end.saturating_sub(start))
        {
            let selected = index == selected_index;
            let current = state.session_id.as_deref() == Some(choice.session_id.as_str());
            rows.push(render_session_picker_choice_row(
                choice,
                selected,
                current,
                inner_width,
            ));
        }
        if end < total {
            rows.push(vec![StyledSegment {
                text: "    ...".to_string(),
                style: SegmentStyle::dim(),
            }]);
        }
        if let Some(choice) = state.session_choices().get(selected_index) {
            rows.extend(render_session_choice_detail_rows(
                choice,
                state.session_id.as_deref() == Some(choice.session_id.as_str()),
                inner_width,
            ));
        }
    }
    let mut lines = vec![StyledLine::plain("")];
    lines.extend(with_codex_border(rows, inner_width));
    lines
}

fn render_session_picker_choice_row(
    choice: &SessionChoice,
    selected: bool,
    current: bool,
    inner_width: usize,
) -> Vec<StyledSegment> {
    let id_width = 24usize.min(inner_width.saturating_sub(18).max(12));
    let short_id = take_prefix_width(&choice.session_id, id_width);
    let meta = format!(
        "  {} runs={} status={} updated={}",
        choice.profile.as_deref().unwrap_or("-"),
        choice.run_count,
        choice.status,
        compact_timestamp(&choice.updated_at)
    );
    vec![
        StyledSegment {
            text: if selected { "> " } else { "  " }.to_string(),
            style: if selected {
                SegmentStyle::warning().merge(SegmentStyle::bold())
            } else {
                SegmentStyle::dim()
            },
        },
        StyledSegment {
            text: if current { "* " } else { "  " }.to_string(),
            style: if current {
                SegmentStyle::blockquote().merge(SegmentStyle::bold())
            } else {
                SegmentStyle::dim()
            },
        },
        StyledSegment {
            text: format!("{short_id:<id_width$}"),
            style: if selected {
                SegmentStyle::bold()
            } else {
                SegmentStyle::default()
            },
        },
        StyledSegment {
            text: format!("  {}", choice.display_title()),
            style: SegmentStyle::default(),
        },
        StyledSegment {
            text: meta,
            style: SegmentStyle::dim(),
        },
    ]
}

fn render_session_choice_detail_rows(
    choice: &SessionChoice,
    current: bool,
    inner_width: usize,
) -> Vec<Vec<StyledSegment>> {
    let mut rows = vec![
        Vec::new(),
        vec![StyledSegment {
            text: "Highlighted session".to_string(),
            style: SegmentStyle::code().merge(SegmentStyle::bold()),
        }],
    ];
    let session_id = if current {
        format!("{} (current)", choice.session_id)
    } else {
        choice.session_id.clone()
    };
    push_detail_row(
        &mut rows,
        "session:",
        &session_id,
        inner_width,
        SegmentStyle::default(),
    );
    push_detail_row(
        &mut rows,
        "title:",
        choice.display_title(),
        inner_width,
        SegmentStyle::default(),
    );
    push_detail_row(
        &mut rows,
        "profile:",
        choice.profile.as_deref().unwrap_or("unknown"),
        inner_width,
        SegmentStyle::default(),
    );
    push_detail_row(
        &mut rows,
        "status:",
        &choice.status,
        inner_width,
        SegmentStyle::default(),
    );
    let runs = choice.run_count.to_string();
    push_detail_row(
        &mut rows,
        "runs:",
        &runs,
        inner_width,
        SegmentStyle::default(),
    );
    push_detail_row(
        &mut rows,
        "updated:",
        &choice.updated_at,
        inner_width,
        SegmentStyle::dim(),
    );
    if let Some(preview) = choice.last_output_preview.as_deref() {
        push_detail_row(
            &mut rows,
            "preview:",
            preview,
            inner_width,
            SegmentStyle::dim(),
        );
    }
    rows
}

pub(super) fn render_model_picker_panel(
    state: &InteractiveTuiState,
    width: usize,
) -> Vec<StyledLine> {
    if width < 4 {
        return Vec::new();
    }
    let inner_width = width.saturating_sub(4);
    let mut rows = Vec::<Vec<StyledSegment>>::new();
    rows.push(vec![
        StyledSegment {
            text: "Model Profiles".to_string(),
            style: SegmentStyle::code().merge(SegmentStyle::bold()),
        },
        StyledSegment {
            text: "  Enter: select | Esc: close | Up/Down: navigate".to_string(),
            style: SegmentStyle::dim(),
        },
    ]);
    if state.model_choices().is_empty() {
        rows.push(vec![StyledSegment {
            text: "No model profiles are configured.".to_string(),
            style: SegmentStyle::dim(),
        }]);
    } else {
        let max_visible = 8usize;
        let total = state.model_choices().len();
        let selected_index = state.model_picker_index().min(total.saturating_sub(1));
        let start = selected_index
            .saturating_sub(max_visible / 2)
            .min(total.saturating_sub(max_visible));
        let end = total.min(start.saturating_add(max_visible));
        if start > 0 {
            rows.push(vec![StyledSegment {
                text: "    ...".to_string(),
                style: SegmentStyle::dim(),
            }]);
        }
        for (index, choice) in state
            .model_choices()
            .iter()
            .enumerate()
            .skip(start)
            .take(end.saturating_sub(start))
        {
            let selected = index == selected_index;
            let current = choice.profile == state.profile;
            rows.push(render_model_picker_choice_row(choice, selected, current));
        }
        if end < total {
            rows.push(vec![StyledSegment {
                text: "    ...".to_string(),
                style: SegmentStyle::dim(),
            }]);
        }
        if let Some(choice) = state.model_choices().get(selected_index) {
            rows.extend(render_model_choice_detail_rows(
                choice,
                choice.profile == state.profile,
                inner_width,
            ));
        }
    }
    let mut lines = vec![StyledLine::plain("")];
    lines.extend(with_codex_border(rows, inner_width));
    lines
}

fn render_model_picker_choice_row(
    choice: &ModelChoice,
    selected: bool,
    current: bool,
) -> Vec<StyledSegment> {
    let label = if choice.display_name() == choice.profile {
        String::new()
    } else {
        format!("  {}", choice.display_name())
    };
    vec![
        StyledSegment {
            text: if selected { "> " } else { "  " }.to_string(),
            style: if selected {
                SegmentStyle::warning().merge(SegmentStyle::bold())
            } else {
                SegmentStyle::dim()
            },
        },
        StyledSegment {
            text: if current { "* " } else { "  " }.to_string(),
            style: if current {
                SegmentStyle::blockquote().merge(SegmentStyle::bold())
            } else {
                SegmentStyle::dim()
            },
        },
        StyledSegment {
            text: format!("{:<18}", choice.profile),
            style: if selected {
                SegmentStyle::bold()
            } else {
                SegmentStyle::default()
            },
        },
        StyledSegment {
            text: label,
            style: SegmentStyle::dim(),
        },
    ]
}

fn render_model_choice_detail_rows(
    choice: &ModelChoice,
    current: bool,
    inner_width: usize,
) -> Vec<Vec<StyledSegment>> {
    let mut rows = vec![
        Vec::new(),
        vec![StyledSegment {
            text: "Highlighted config".to_string(),
            style: SegmentStyle::code().merge(SegmentStyle::bold()),
        }],
    ];
    let profile = if current {
        format!("{} (current)", choice.profile)
    } else {
        choice.profile.clone()
    };
    push_detail_row(
        &mut rows,
        "profile:",
        &profile,
        inner_width,
        SegmentStyle::default(),
    );
    if choice.display_name() != choice.profile {
        push_detail_row(
            &mut rows,
            "label:",
            choice.display_name(),
            inner_width,
            SegmentStyle::default(),
        );
    }
    push_detail_row(
        &mut rows,
        "model:",
        &choice.model_id,
        inner_width,
        SegmentStyle::default(),
    );
    let model_settings = choice.model_settings.as_deref().unwrap_or("default");
    push_detail_row(
        &mut rows,
        "model_settings:",
        model_settings,
        inner_width,
        SegmentStyle::default(),
    );
    let model_cfg = choice.model_cfg.as_deref().unwrap_or("default");
    push_detail_row(
        &mut rows,
        "model_cfg:",
        model_cfg,
        inner_width,
        SegmentStyle::default(),
    );
    let context = choice.context_window.map_or_else(
        || "unknown".to_string(),
        |window| format!("{window} tokens"),
    );
    push_detail_row(
        &mut rows,
        "context:",
        &context,
        inner_width,
        SegmentStyle::default(),
    );
    push_detail_row(
        &mut rows,
        "source:",
        &choice.source,
        inner_width,
        SegmentStyle::dim(),
    );
    rows
}

pub(super) fn push_detail_row(
    rows: &mut Vec<Vec<StyledSegment>>,
    label: &str,
    value: &str,
    inner_width: usize,
    value_style: SegmentStyle,
) {
    let label_text = format!("  {label:<15}");
    let value_width = inner_width
        .saturating_sub(visible_width(&label_text))
        .max(1);
    let continuation = " ".repeat(visible_width(&label_text));
    for (index, line) in wrap_text_width(value, value_width).into_iter().enumerate() {
        rows.push(vec![
            StyledSegment {
                text: if index == 0 {
                    label_text.clone()
                } else {
                    continuation.clone()
                },
                style: SegmentStyle::dim(),
            },
            StyledSegment {
                text: line,
                style: value_style,
            },
        ]);
    }
}
