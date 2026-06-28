use super::{full_content_lines, preview_line, preview_lines, Value};

pub(in crate::tui::state) fn format_custom_context_event_lines(
    kind: &str,
    payload: &Value,
) -> Option<Vec<String>> {
    let normalized = normalized_event_kind(kind);
    let context_payload = payload.get("payload").unwrap_or(payload);
    if event_kind_matches(
        &normalized,
        &[
            "compact_start",
            "compact_started",
            "compaction_start",
            "compaction_started",
        ],
    ) {
        return Some(format_compaction_started_lines(context_payload));
    }
    if event_kind_matches(
        &normalized,
        &[
            "compact_complete",
            "compact_completed",
            "compaction_complete",
            "compaction_completed",
        ],
    ) {
        return Some(format_compaction_completed_lines(context_payload, payload));
    }
    if event_kind_matches(
        &normalized,
        &[
            "compact_failed",
            "compact_failure",
            "compaction_failed",
            "compaction_failure",
        ],
    ) {
        return Some(vec![format!(
            "Compact failed: {}",
            compact_error_text(context_payload, payload)
        )]);
    }
    if event_kind_matches(
        &normalized,
        &[
            "handoff_start",
            "handoff_started",
            "summary_start",
            "summary_started",
        ],
    ) {
        return Some(format_handoff_started_lines(context_payload));
    }
    if event_kind_matches(
        &normalized,
        &[
            "handoff_complete",
            "handoff_completed",
            "summary_complete",
            "summary_completed",
        ],
    ) {
        return Some(format_handoff_completed_lines(context_payload, payload));
    }
    if event_kind_matches(
        &normalized,
        &[
            "handoff_failed",
            "handoff_failure",
            "summary_failed",
            "summary_failure",
        ],
    ) {
        return Some(vec![format!(
            "Summary failed: {}",
            compact_error_text(context_payload, payload)
        )]);
    }
    if event_kind_matches(&normalized, &["goal_iteration"]) {
        return Some(format_goal_iteration_lines(context_payload));
    }
    if event_kind_matches(&normalized, &["goal_complete", "goal_completed"]) {
        return Some(format_goal_complete_lines(context_payload));
    }
    None
}

pub(in crate::tui::state) fn normalized_event_kind(kind: &str) -> String {
    kind.to_ascii_lowercase().replace(['.', '-'], "_")
}

pub(in crate::tui::state) fn is_subagent_start_event_kind(normalized: &str) -> bool {
    event_kind_matches(normalized, &["subagent_start", "subagent_started"])
}

pub(in crate::tui::state) fn is_subagent_complete_event_kind(normalized: &str) -> bool {
    event_kind_matches(normalized, &["subagent_complete", "subagent_completed"])
}

pub(in crate::tui::state) fn is_subagent_failed_event_kind(normalized: &str) -> bool {
    event_kind_matches(normalized, &["subagent_fail", "subagent_failed"])
}

pub(in crate::tui::state) fn is_subagent_lifecycle_event_kind(kind: &str) -> bool {
    let normalized = normalized_event_kind(kind);
    is_subagent_start_event_kind(&normalized)
        || is_subagent_complete_event_kind(&normalized)
        || is_subagent_failed_event_kind(&normalized)
}

fn event_kind_matches(normalized: &str, candidates: &[&str]) -> bool {
    candidates
        .iter()
        .any(|candidate| normalized == *candidate || normalized.ends_with(&format!("_{candidate}")))
}

pub(in crate::tui::state) fn subagent_event_payload(payload: &Value) -> &Value {
    payload.get("payload").unwrap_or(payload)
}

pub(in crate::tui::state) fn subagent_display_id(payload: &Value) -> String {
    let payload = subagent_event_payload(payload);
    payload_string(
        payload,
        &[
            "agent_id",
            "id",
            "name",
            "subagent",
            "subagent_name",
            "agent_name",
            "agent",
        ],
    )
    .unwrap_or_else(|| "subagent".to_string())
}

pub(in crate::tui::state) fn format_subagent_running_line(payload: &Value) -> String {
    let agent_id = subagent_display_id(payload);
    format!("[{agent_id}] Running...")
}

pub(in crate::tui::state) fn format_subagent_finished_line(kind: &str, payload: &Value) -> String {
    let normalized = normalized_event_kind(kind);
    let payload = subagent_event_payload(payload);
    let agent_id = subagent_display_id(payload);
    let success = if is_subagent_failed_event_kind(&normalized) {
        false
    } else {
        payload
            .get("success")
            .and_then(Value::as_bool)
            .unwrap_or(true)
    };

    let mut line = if success {
        format!("[{agent_id}] Done")
    } else {
        format!("[{agent_id}] Failed")
    };
    if let Some(duration_seconds) = payload_f64(
        payload,
        &["duration_seconds", "duration", "elapsed_seconds"],
    ) {
        line.push_str(" (");
        let duration_text = format!("{duration_seconds:.1}s");
        line.push_str(&duration_text);
        line.push(')');
    }
    if success {
        if let Some(request_count) =
            payload_u64(payload, &["request_count", "requests"]).filter(|count| *count > 0)
        {
            line.push_str(" | ");
            line.push_str(&request_count.to_string());
            line.push_str(" reqs");
        }
        if let Some(preview) = payload_string(payload, &["result_preview", "preview", "output"])
            .filter(|preview| !preview.trim().is_empty())
        {
            line.push_str(" | \"");
            line.push_str(&preview_line(&preview.replace('\n', " ")));
            line.push('"');
        }
    } else if let Some(error) = payload
        .get("metadata")
        .and_then(|metadata| payload_string(metadata, &["error", "message", "reason"]))
        .or_else(|| payload_string(payload, &["error", "message", "reason"]))
    {
        line.push_str(" | ");
        line.push_str(&preview_line(&error));
    }
    line
}

fn format_compaction_started_lines(payload: &Value) -> Vec<String> {
    let message_count = payload_u64(
        payload,
        &[
            "message_count",
            "messages",
            "original_count",
            "original_message_count",
            "original_messages",
        ],
    );
    vec![message_count.map_or_else(
        || "Context compacting...".to_string(),
        |count| format!("Context compacting {count} messages..."),
    )]
}

fn format_compaction_completed_lines(payload: &Value, wrapper: &Value) -> Vec<String> {
    let mut lines = vec!["Context compacted".to_string()];
    let original = payload_u64(
        payload,
        &[
            "original_count",
            "original_message_count",
            "original_messages",
        ],
    );
    let compacted = payload_u64(
        payload,
        &[
            "compacted_count",
            "compacted_message_count",
            "compacted_messages",
            "message_count",
            "messages",
        ],
    );
    match (original, compacted) {
        (Some(original), Some(compacted)) if original > 0 => {
            let reduction = original
                .saturating_sub(compacted)
                .saturating_mul(100)
                .saturating_add(original / 2)
                / original;
            lines.push(format!(
                "  Summary: {original} -> {compacted} messages ({reduction}% reduction)"
            ));
        }
        (_, Some(compacted)) => {
            if let Some(revision) = payload_u64(payload, &["revision"]) {
                lines.push(format!(
                    "  Summary: revision {revision}, {compacted} messages retained"
                ));
            } else {
                lines.push(format!("  Summary: {compacted} messages retained"));
            }
        }
        _ => {
            let preview = payload_string(wrapper, &["preview"])
                .unwrap_or_else(|| "Context compaction completed".to_string());
            lines.push(format!("  Summary: {}", preview_line(&preview)));
        }
    }
    if let Some(summary) = payload_string(
        payload,
        &["summary", "summary_markdown", "content", "handoff_content"],
    ) {
        push_indented_full_content(&mut lines, &summary);
    }
    lines
}

fn format_handoff_started_lines(payload: &Value) -> Vec<String> {
    let message_count = payload_u64(
        payload,
        &[
            "message_count",
            "messages",
            "original_count",
            "original_message_count",
        ],
    );
    vec![message_count.map_or_else(
        || "Summarizing progress...".to_string(),
        |count| format!("Summarizing progress ({count} messages)..."),
    )]
}

fn format_handoff_completed_lines(payload: &Value, wrapper: &Value) -> Vec<String> {
    let mut lines = vec!["Summary complete".to_string()];
    lines.push("  Summary: Progress summarized, continuing with fresh context".to_string());
    let content = payload_string(
        payload,
        &["handoff_content", "content", "summary_markdown", "summary"],
    )
    .or_else(|| payload_string(wrapper, &["preview"]));
    if let Some(content) = content {
        push_indented_full_content(&mut lines, &content);
    }
    lines
}

pub(super) fn push_indented_preview(lines: &mut Vec<String>, content: &str, max_lines: usize) {
    if content.trim().is_empty() {
        return;
    }
    for line in preview_lines(content, max_lines) {
        lines.push(format!("    │ {line}"));
    }
}

pub(super) fn push_indented_full_content(lines: &mut Vec<String>, content: &str) {
    if content.trim().is_empty() {
        return;
    }
    for line in full_content_lines(content) {
        lines.push(format!("    │ {line}"));
    }
}

fn compact_error_text(payload: &Value, wrapper: &Value) -> String {
    payload_string(payload, &["error", "message", "reason"])
        .or_else(|| payload_string(wrapper, &["error", "message", "preview"]))
        .unwrap_or_else(|| "unknown error".to_string())
}

fn format_goal_iteration_lines(payload: &Value) -> Vec<String> {
    let iteration = payload_u64(payload, &["iteration"]).unwrap_or(0);
    let max_iterations = payload_u64(payload, &["max_iterations"]).unwrap_or(0);
    vec![format!("[Goal] Iteration {iteration}/{max_iterations}")]
}

fn format_goal_complete_lines(payload: &Value) -> Vec<String> {
    let reason = payload_string(payload, &["reason"]).unwrap_or_else(|| "unknown".to_string());
    let iteration = payload_u64(payload, &["iteration"]).unwrap_or(0);
    let line = match reason.as_str() {
        "verified" => format!("[Goal] Completed: verified after {iteration} iteration(s)"),
        "max_iterations" => {
            format!("[Goal] Completed: max iterations reached after {iteration} iteration(s)")
        }
        other => format!("[Goal] Completed: {other}"),
    };
    vec![line]
}

fn payload_u64(value: &Value, keys: &[&str]) -> Option<u64> {
    keys.iter().find_map(|key| {
        value.get(*key).and_then(|item| {
            item.as_u64()
                .or_else(|| item.as_i64().and_then(|number| u64::try_from(number).ok()))
                .or_else(|| item.as_str().and_then(|text| text.parse::<u64>().ok()))
        })
    })
}

fn payload_f64(value: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|key| {
        value.get(*key).and_then(|item| match item {
            Value::Number(number) => number.to_string().parse::<f64>().ok(),
            Value::String(text) => text.parse::<f64>().ok(),
            _ => None,
        })
    })
}

pub(super) fn payload_string(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        let item = value.get(*key)?;
        match item {
            Value::String(text) if !text.trim().is_empty() => Some(text.clone()),
            Value::Null => None,
            other if !other.to_string().trim().is_empty() => Some(other.to_string()),
            _ => None,
        }
    })
}

pub(super) fn payload_string_array(value: &Value, key: &str) -> Option<Vec<String>> {
    let item = value.get(key)?;
    let values = match item {
        Value::Array(items) => items
            .iter()
            .filter_map(|item| item.as_str().map(str::to_string))
            .collect::<Vec<_>>(),
        Value::String(text) if !text.trim().is_empty() => vec![text.clone()],
        _ => Vec::new(),
    };
    (!values.is_empty()).then_some(values)
}
