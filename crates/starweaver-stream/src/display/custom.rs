//! Custom runtime event display projection.

use serde_json::Value;
use starweaver_context::TASK_SNAPSHOT_EVENT_KIND;
use starweaver_core::{Metadata, RunId};

use super::{DisplayMessage, DisplayMessageKind, DisplayProjectionContext};

pub(super) fn project_custom_event(
    context: &DisplayProjectionContext,
    sequence: usize,
    run_id: RunId,
    kind: &str,
    payload: &Value,
    metadata: &Metadata,
) -> Vec<DisplayMessage> {
    let normalized = kind.to_ascii_lowercase().replace(['.', '-'], "_");
    let Some(display_kind) = custom_display_kind(&normalized) else {
        return Vec::new();
    };
    let preview = custom_display_preview(display_kind, payload);
    let mut display =
        DisplayMessage::new(sequence, context.session_id.clone(), run_id, display_kind)
            .with_payload(payload.clone())
            .with_preview(preview);
    display.metadata.clone_from(metadata);
    display
        .metadata
        .entry("starweaver_event_kind".to_string())
        .or_insert_with(|| Value::String(kind.to_string()));
    vec![display]
}

#[allow(clippy::too_many_lines)]
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
    } else if custom_event_kind_matches(normalized, &["goal_iteration"]) {
        Some(DisplayMessageKind::GoalIteration)
    } else if custom_event_kind_matches(normalized, &["goal_complete", "goal_completed"]) {
        Some(DisplayMessageKind::GoalCompleted)
    } else if custom_event_kind_matches(
        normalized,
        &["model_transport_selected", "model_transport_fallback"],
    ) {
        Some(DisplayMessageKind::HostEvent)
    } else if custom_event_kind_matches(normalized, &["tools_unavailable"]) {
        Some(DisplayMessageKind::ToolsUnavailable)
    } else if custom_event_kind_matches(normalized, &["tool_search_loaded"]) {
        Some(DisplayMessageKind::ToolSearchLoaded)
    } else if custom_event_kind_matches(normalized, &["tool_search_initialized"]) {
        Some(DisplayMessageKind::ToolSearchInitialized)
    } else if custom_event_kind_matches(normalized, &["tool_search_refreshed"]) {
        Some(DisplayMessageKind::ToolSearchRefreshed)
    } else if custom_event_kind_matches(normalized, &["tool_search_invalidated"]) {
        Some(DisplayMessageKind::ToolSearchInvalidated)
    } else if custom_event_kind_matches(normalized, &["tool_search_failed"]) {
        Some(DisplayMessageKind::ToolSearchFailed)
    } else if custom_event_kind_matches(normalized, &["tool_search_no_match"]) {
        Some(DisplayMessageKind::ToolSearchNoMatch)
    } else if custom_event_kind_matches(normalized, &["toolset_initialized"]) {
        Some(DisplayMessageKind::ToolsetInitialized)
    } else if custom_event_kind_matches(normalized, &["toolset_unavailable"]) {
        Some(DisplayMessageKind::ToolsetUnavailable)
    } else if custom_event_kind_matches(normalized, &["toolset_failed"]) {
        Some(DisplayMessageKind::ToolsetFailed)
    } else if custom_event_kind_matches(normalized, &["toolset_refreshed"]) {
        Some(DisplayMessageKind::ToolsetRefreshed)
    } else if custom_event_kind_matches(normalized, &["toolset_closed"]) {
        Some(DisplayMessageKind::ToolsetClosed)
    } else if custom_event_kind_matches(normalized, &["approval_requested"]) {
        Some(DisplayMessageKind::ApprovalRequested)
    } else if custom_event_kind_matches(normalized, &["approval_resolved"]) {
        Some(DisplayMessageKind::ApprovalResolved)
    } else if custom_event_kind_matches(
        normalized,
        &[
            "hitl_resolved",
            "deferred_completed",
            "deferred_failed",
            "deferred_cancelled",
        ],
    ) {
        Some(DisplayMessageKind::HitlResolved)
    } else if custom_event_kind_matches(
        normalized,
        &[
            "hitl_decision_diagnostic",
            "hitl_resolution_failed",
            "hitl_diagnostic",
        ],
    ) {
        Some(DisplayMessageKind::HitlDiagnostic)
    } else if custom_event_kind_matches(normalized, &["skills_scanned"]) {
        Some(DisplayMessageKind::SkillsScanned)
    } else if custom_event_kind_matches(normalized, &["skill_activated"]) {
        Some(DisplayMessageKind::SkillActivated)
    } else if custom_event_kind_matches(normalized, &["skills_reloaded"]) {
        Some(DisplayMessageKind::SkillsReloaded)
    } else if custom_event_kind_matches(normalized, &["subagent_started"]) {
        Some(DisplayMessageKind::SubagentStarted)
    } else if custom_event_kind_matches(normalized, &["subagent_completed"]) {
        Some(DisplayMessageKind::SubagentCompleted)
    } else if custom_event_kind_matches(normalized, &["subagent_failed", "subagent_fail"]) {
        Some(DisplayMessageKind::SubagentFailed)
    } else if custom_event_kind_matches(normalized, &[TASK_SNAPSHOT_EVENT_KIND, "task_snapshot"]) {
        Some(DisplayMessageKind::TaskSnapshot)
    } else if normalized.starts_with("task_") {
        Some(DisplayMessageKind::TaskEvent)
    } else if normalized.starts_with("note_") {
        Some(DisplayMessageKind::NoteEvent)
    } else if normalized.starts_with("file_") {
        Some(DisplayMessageKind::FileEvent)
    } else if normalized.starts_with("media_") {
        Some(DisplayMessageKind::MediaEvent)
    } else if normalized.starts_with("host_") {
        Some(DisplayMessageKind::HostEvent)
    } else {
        None
    }
}

fn custom_event_kind_matches(normalized: &str, candidates: &[&str]) -> bool {
    candidates
        .iter()
        .any(|candidate| normalized == *candidate || normalized.ends_with(&format!("_{candidate}")))
}

#[allow(clippy::too_many_lines)]
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
        DisplayMessageKind::GoalIteration => {
            let iteration = payload_u64(payload, &["iteration"]).unwrap_or(0);
            let max_iterations = payload_u64(payload, &["max_iterations"]).unwrap_or(0);
            format!("goal iteration {iteration}/{max_iterations}")
        }
        DisplayMessageKind::GoalCompleted => payload_string(payload, &["reason"]).map_or_else(
            || "goal completed".to_string(),
            |reason| format!("goal completed: {reason}"),
        ),
        DisplayMessageKind::TaskSnapshot => task_snapshot_items(payload).map_or_else(
            || "task snapshot".to_string(),
            |tasks| format!("task snapshot: {} task(s)", tasks.len()),
        ),
        DisplayMessageKind::TaskEvent => generic_event_preview("task event", payload),
        DisplayMessageKind::NoteEvent => generic_event_preview("note event", payload),
        DisplayMessageKind::FileEvent => generic_event_preview("file event", payload),
        DisplayMessageKind::MediaEvent => generic_event_preview("media event", payload),
        DisplayMessageKind::HostEvent => model_transport_preview(payload)
            .unwrap_or_else(|| generic_event_preview("host event", payload)),
        DisplayMessageKind::ToolsUnavailable => payload_array_len(payload, &["unavailable"])
            .map_or_else(
                || "tools unavailable".to_string(),
                |count| format!("tools unavailable: {count} tool(s)"),
            ),
        DisplayMessageKind::ToolSearchLoaded => {
            let tools = payload_array_len(payload, &["loaded_tools"]).unwrap_or(0);
            let namespaces = payload_array_len(payload, &["loaded_namespaces"]).unwrap_or(0);
            format!("tool search loaded {tools} tool(s), {namespaces} namespace(s)")
        }
        DisplayMessageKind::ToolSearchInitialized => {
            tool_search_report_preview("initialized", payload)
        }
        DisplayMessageKind::ToolSearchRefreshed => tool_search_report_preview("refreshed", payload),
        DisplayMessageKind::ToolSearchInvalidated => {
            let tools = payload_array_len(payload, &["removed_loaded_tools"]).unwrap_or(0);
            let namespaces =
                payload_array_len(payload, &["removed_loaded_namespaces"]).unwrap_or(0);
            format!("tool search invalidated {tools} tool(s), {namespaces} namespace(s)")
        }
        DisplayMessageKind::ToolSearchFailed => payload_string(payload, &["message"]).map_or_else(
            || "tool search failed".to_string(),
            |message| format!("tool search failed: {message}"),
        ),
        DisplayMessageKind::ToolSearchNoMatch => payload_string(payload, &["query"]).map_or_else(
            || "tool search no match".to_string(),
            |query| format!("tool search no match: {query}"),
        ),
        DisplayMessageKind::ToolsetInitialized => toolset_lifecycle_preview("initialized", payload),
        DisplayMessageKind::ToolsetUnavailable => toolset_lifecycle_preview("unavailable", payload),
        DisplayMessageKind::ToolsetFailed => toolset_lifecycle_preview("failed", payload),
        DisplayMessageKind::ToolsetRefreshed => toolset_lifecycle_preview("refreshed", payload),
        DisplayMessageKind::ToolsetClosed => toolset_lifecycle_preview("closed", payload),
        DisplayMessageKind::ApprovalRequested => payload_string(payload, &["tool_name", "name"])
            .map_or_else(
                || "approval requested".to_string(),
                |tool| format!("approval requested: {tool}"),
            ),
        DisplayMessageKind::ApprovalResolved => payload_string(payload, &["status", "decision"])
            .map_or_else(
                || "approval resolved".to_string(),
                |status| format!("approval resolved: {status}"),
            ),
        DisplayMessageKind::HitlResolved => hitl_resolved_preview(payload),
        DisplayMessageKind::HitlDiagnostic => hitl_diagnostic_preview(payload),
        DisplayMessageKind::SkillsScanned => skill_report_preview("skills scanned", payload, false),
        DisplayMessageKind::SkillActivated => payload_string(payload, &["name"]).map_or_else(
            || "skill activated".to_string(),
            |name| format!("skill activated: {name}"),
        ),
        DisplayMessageKind::SkillsReloaded => {
            skill_report_preview("skills reloaded", payload, true)
        }
        DisplayMessageKind::SubagentStarted => subagent_preview("subagent started", payload),
        DisplayMessageKind::SubagentCompleted => subagent_preview("subagent completed", payload),
        DisplayMessageKind::SubagentFailed => {
            let base = subagent_preview("subagent failed", payload);
            payload_string(payload, &["error", "message"])
                .or_else(|| payload_nested_string(payload, &["metadata", "error"]))
                .map_or_else(|| base.clone(), |error| format!("{base}: {error}"))
        }
        _ => "custom display event".to_string(),
    }
}

fn task_snapshot_items(value: &Value) -> Option<&Vec<Value>> {
    value
        .get("tasks")
        .and_then(Value::as_array)
        .or_else(|| value.as_array())
}

fn tool_search_report_preview(action: &str, payload: &Value) -> String {
    let tools = payload_u64(payload, &["total_tools"])
        .or_else(|| {
            payload_array_len(payload, &["loose_tools"]).and_then(|count| count.try_into().ok())
        })
        .unwrap_or(0);
    let namespaces = payload_u64(payload, &["total_namespaces"])
        .or_else(|| {
            payload_array_len(payload, &["namespaces"]).and_then(|count| count.try_into().ok())
        })
        .unwrap_or(0);
    format!("tool search {action}: {tools} tool(s), {namespaces} namespace(s)")
}

fn toolset_lifecycle_preview(action: &str, payload: &Value) -> String {
    let name =
        payload_string(payload, &["name", "toolset"]).unwrap_or_else(|| "toolset".to_string());
    let tool_count = payload_u64(payload, &["tool_count"]).unwrap_or(0);
    payload_string(payload, &["message", "error"]).map_or_else(
        || format!("{name} {action}: {tool_count} tool(s)"),
        |message| format!("{name} {action}: {message}"),
    )
}

fn skill_report_preview(label: &str, payload: &Value, include_changes: bool) -> String {
    let packages = payload_u64(payload, &["package_count"])
        .or_else(|| {
            payload_array_len(payload, &["packages"]).and_then(|count| count.try_into().ok())
        })
        .unwrap_or(0);
    let diagnostics = payload_array_len(payload, &["diagnostics"]).unwrap_or(0);
    if include_changes {
        let changes = payload_array_len(payload, &["changes"]).unwrap_or(0);
        format!("{label}: {packages} package(s), {changes} change(s), {diagnostics} diagnostic(s)")
    } else {
        format!("{label}: {packages} package(s), {diagnostics} diagnostic(s)")
    }
}

fn hitl_resolved_preview(payload: &Value) -> String {
    let returns = payload_u64(payload, &["tool_returns"]).unwrap_or(0);
    let approved = payload_u64(payload, &["approved"]).unwrap_or(0);
    let denied = payload_u64(payload, &["denied"]).unwrap_or(0);
    let deferred_completed = payload_u64(payload, &["deferred_completed"]).unwrap_or(0);
    let deferred_failed = payload_u64(payload, &["deferred_failed"]).unwrap_or(0);
    let deferred_cancelled = payload_u64(payload, &["deferred_cancelled"]).unwrap_or(0);
    format!(
        "hitl resolved: {returns} return(s), {approved} approved, {denied} denied, {deferred_completed} deferred completed, {deferred_failed} deferred failed, {deferred_cancelled} deferred cancelled"
    )
}

fn hitl_diagnostic_preview(payload: &Value) -> String {
    let kind = payload_string(payload, &["error_kind"]).unwrap_or_else(|| "error".to_string());
    payload_string(
        payload,
        &["decision_id", "approval_id", "deferred_id", "tool_call_id"],
    )
    .or_else(|| payload_string(payload, &["message"]))
    .map_or_else(
        || format!("hitl diagnostic: {kind}"),
        |detail| format!("hitl diagnostic: {kind}: {detail}"),
    )
}

fn subagent_preview(label: &str, payload: &Value) -> String {
    payload_string(
        payload,
        &["name", "subagent", "subagent_name", "agent_name"],
    )
    .map_or_else(|| label.to_string(), |name| format!("{label}: {name}"))
}

fn model_transport_preview(payload: &Value) -> Option<String> {
    payload_string(payload, &["message"])
}

fn generic_event_preview(label: &str, payload: &Value) -> String {
    payload_string(
        payload,
        &[
            "title",
            "subject",
            "name",
            "path",
            "uri",
            "operation",
            "status",
        ],
    )
    .map_or_else(|| label.to_string(), |value| format!("{label}: {value}"))
}

fn payload_array_len(value: &Value, keys: &[&str]) -> Option<usize> {
    keys.iter().find_map(|key| {
        value.get(*key).and_then(|item| {
            item.as_array()
                .map(Vec::len)
                .or_else(|| item.as_object().map(serde_json::Map::len))
                .or_else(|| {
                    item.as_u64()
                        .and_then(|number| usize::try_from(number).ok())
                })
                .or_else(|| {
                    item.as_i64()
                        .and_then(|number| usize::try_from(number).ok())
                })
        })
    })
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

fn payload_nested_string(value: &Value, path: &[&str]) -> Option<String> {
    let mut cursor = value;
    for key in path {
        cursor = cursor.get(*key)?;
    }
    match cursor {
        Value::String(text) if !text.trim().is_empty() => Some(text.clone()),
        Value::Null => None,
        other if !other.to_string().trim().is_empty() => Some(other.to_string()),
        _ => None,
    }
}
