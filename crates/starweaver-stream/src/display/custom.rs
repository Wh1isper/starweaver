//! Custom runtime event display projection.

use serde_json::Value;
use starweaver_context::TASK_SNAPSHOT_EVENT_KIND;
use starweaver_core::RunId;

use super::{DisplayMessage, DisplayMessageKind, DisplayProjectionContext};

pub(super) fn project_custom_event(
    context: &DisplayProjectionContext,
    sequence: usize,
    run_id: RunId,
    kind: &str,
    payload: &Value,
) -> Vec<DisplayMessage> {
    let normalized = kind.to_ascii_lowercase().replace(['.', '-'], "_");
    let Some(display_kind) = custom_display_kind(&normalized) else {
        return Vec::new();
    };
    let preview = custom_display_preview(display_kind, payload);
    vec![
        DisplayMessage::new(sequence, context.session_id.clone(), run_id, display_kind)
            .with_payload(payload.clone())
            .with_preview(preview),
    ]
}

fn custom_display_kind(normalized: &str) -> Option<DisplayMessageKind> {
    if custom_event_kind_matches(
        normalized,
        &[
            "compact_start",
            "compact_started",
            "compaction_start",
            "compaction_started",
        ],
    ) {
        Some(DisplayMessageKind::CompactionStarted)
    } else if custom_event_kind_matches(
        normalized,
        &[
            "compact_complete",
            "compact_completed",
            "compaction_complete",
            "compaction_completed",
        ],
    ) {
        Some(DisplayMessageKind::CompactionCompleted)
    } else if custom_event_kind_matches(
        normalized,
        &[
            "compact_failed",
            "compact_failure",
            "compaction_failed",
            "compaction_failure",
        ],
    ) {
        Some(DisplayMessageKind::CompactionFailed)
    } else if custom_event_kind_matches(
        normalized,
        &[
            "handoff_start",
            "handoff_started",
            "summary_start",
            "summary_started",
        ],
    ) {
        Some(DisplayMessageKind::HandoffStarted)
    } else if custom_event_kind_matches(
        normalized,
        &[
            "handoff_complete",
            "handoff_completed",
            "summary_complete",
            "summary_completed",
        ],
    ) {
        Some(DisplayMessageKind::HandoffCompleted)
    } else if custom_event_kind_matches(
        normalized,
        &[
            "handoff_failed",
            "handoff_failure",
            "summary_failed",
            "summary_failure",
        ],
    ) {
        Some(DisplayMessageKind::HandoffFailed)
    } else if custom_event_kind_matches(normalized, &["steering_submitted", "steer_submitted"]) {
        Some(DisplayMessageKind::SteeringSubmitted)
    } else if custom_event_kind_matches(
        normalized,
        &[
            "steering_received",
            "steer_received",
            "steering_ack",
            "steer_ack",
        ],
    ) {
        Some(DisplayMessageKind::SteeringReceived)
    } else if custom_event_kind_matches(
        normalized,
        &[TASK_SNAPSHOT_EVENT_KIND, "task_snapshot", "task_panel"],
    ) {
        Some(DisplayMessageKind::TaskSnapshot)
    } else {
        None
    }
}

fn custom_event_kind_matches(normalized: &str, candidates: &[&str]) -> bool {
    candidates
        .iter()
        .any(|candidate| normalized == *candidate || normalized.ends_with(&format!("_{candidate}")))
}

fn custom_display_preview(kind: DisplayMessageKind, payload: &Value) -> String {
    match kind {
        DisplayMessageKind::CompactionStarted => payload_u64(
            payload,
            &[
                "message_count",
                "messages",
                "original_count",
                "original_message_count",
            ],
        )
        .map_or_else(
            || "context compaction started".to_string(),
            |count| format!("context compacting {count} messages"),
        ),
        DisplayMessageKind::CompactionCompleted => "context compaction completed".to_string(),
        DisplayMessageKind::CompactionFailed => payload_string(payload, &["error", "message"])
            .map_or_else(
                || "context compaction failed".to_string(),
                |error| format!("context compaction failed: {error}"),
            ),
        DisplayMessageKind::HandoffStarted => payload_u64(payload, &["message_count", "messages"])
            .map_or_else(
                || "handoff summary started".to_string(),
                |count| format!("summarizing progress ({count} messages)"),
            ),
        DisplayMessageKind::HandoffCompleted => "handoff summary completed".to_string(),
        DisplayMessageKind::HandoffFailed => payload_string(payload, &["error", "message"])
            .map_or_else(
                || "handoff summary failed".to_string(),
                |error| format!("handoff summary failed: {error}"),
            ),
        DisplayMessageKind::SteeringSubmitted => {
            payload_string(payload, &["text", "prompt", "message"]).map_or_else(
                || "steering submitted".to_string(),
                |text| format!("steering submitted: {text}"),
            )
        }
        DisplayMessageKind::SteeringReceived => {
            payload_string(payload, &["text", "prompt", "message"]).map_or_else(
                || "steering received".to_string(),
                |text| format!("steering received: {text}"),
            )
        }
        DisplayMessageKind::TaskSnapshot => task_snapshot_items(payload).map_or_else(
            || "task snapshot".to_string(),
            |tasks| format!("task snapshot: {} task(s)", tasks.len()),
        ),
        _ => "custom display event".to_string(),
    }
}

fn task_snapshot_items(value: &Value) -> Option<&Vec<Value>> {
    value
        .get("tasks")
        .or_else(|| value.get("items"))
        .and_then(Value::as_array)
        .or_else(|| value.as_array())
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
