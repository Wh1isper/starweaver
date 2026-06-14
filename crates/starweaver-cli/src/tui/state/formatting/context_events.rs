use super::{preview_line, preview_lines, Value};

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
    None
}

fn normalized_event_kind(kind: &str) -> String {
    kind.to_ascii_lowercase().replace(['.', '-'], "_")
}

fn event_kind_matches(normalized: &str, candidates: &[&str]) -> bool {
    candidates
        .iter()
        .any(|candidate| normalized == *candidate || normalized.ends_with(&format!("_{candidate}")))
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
        push_indented_preview(&mut lines, &summary, 8);
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
        push_indented_preview(&mut lines, &content, 12);
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

fn compact_error_text(payload: &Value, wrapper: &Value) -> String {
    payload_string(payload, &["error", "message", "reason"])
        .or_else(|| payload_string(wrapper, &["error", "message", "preview"]))
        .unwrap_or_else(|| "unknown error".to_string())
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
