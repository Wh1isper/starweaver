#![allow(clippy::unwrap_used)]

use super::*;
use serde_json::json;
use starweaver_core::SessionId;
use starweaver_core::{AgentId, RunId, TaskId};
use starweaver_model::{
    ModelResponse, ModelResponsePart, ModelResponseStreamEvent, PartDelta, PartEnd, PartStart,
    ProviderPartInfo, ToolCallPart, ToolReturnPart,
};

fn custom_stream_record(
    sequence: usize,
    kind: &str,
    payload: &serde_json::Value,
) -> AgentStreamRecord {
    serde_json::from_value(json!({
        "sequence": sequence,
        "event": {
            "kind": "custom",
            "event": {
                "kind": kind,
                "payload": payload,
            }
        }
    }))
    .unwrap()
}

fn display_message(
    sequence: usize,
    kind: DisplayMessageKind,
    payload: serde_json::Value,
) -> DisplayMessage {
    DisplayMessage::new(
        sequence,
        SessionId::from_string("session-1"),
        RunId::from_string("run-1"),
        kind,
    )
    .with_payload(payload)
}

#[test]
fn display_message_serializes_with_agui_event_name() {
    let message = display_message(
        1,
        DisplayMessageKind::AssistantTextDelta,
        json!({"delta": "hello"}),
    );
    let value = serde_json::to_value(&message).unwrap();
    assert_eq!(value["schema"], "starweaver.display.v1");
    assert_eq!(value["type"], "TEXT_MESSAGE_CONTENT");
    assert_eq!(
        serde_json::from_value::<DisplayMessage>(value).unwrap(),
        message
    );
}

#[test]
fn legacy_host_operation_deserializes_as_host_event() {
    let message = display_message(1, DisplayMessageKind::HostEvent, json!({"action": "open"}));
    let mut legacy = serde_json::to_value(&message).unwrap();
    legacy["type"] = json!("HOST_OPERATION");

    let decoded = serde_json::from_value::<DisplayMessage>(legacy).unwrap();

    assert_eq!(decoded.kind, DisplayMessageKind::HostEvent);
    assert_eq!(
        serde_json::to_value(decoded).unwrap()["type"],
        json!("HOST_EVENT")
    );
}

#[test]
fn replay_display_event_preserves_message_timestamp() {
    let message = display_message(7, DisplayMessageKind::RunCompleted, serde_json::Value::Null);
    let timestamp = message.timestamp;
    let event = ReplayEvent::display(ReplayScope::run("run-1"), message);
    assert_eq!(event.timestamp, timestamp);
}

#[test]
fn all_display_message_kinds_serialize_to_agui_event_names() {
    let cases = [
        (DisplayMessageKind::RunQueued, "RUN_QUEUED"),
        (DisplayMessageKind::RunStarted, "RUN_STARTED"),
        (DisplayMessageKind::AssistantTextStart, "TEXT_MESSAGE_START"),
        (
            DisplayMessageKind::AssistantTextDelta,
            "TEXT_MESSAGE_CONTENT",
        ),
        (DisplayMessageKind::AssistantTextEnd, "TEXT_MESSAGE_END"),
        (DisplayMessageKind::ToolCallStart, "TOOL_CALL_START"),
        (DisplayMessageKind::ToolCallDelta, "TOOL_CALL_ARGS"),
        (DisplayMessageKind::ToolCallEnd, "TOOL_CALL_END"),
        (DisplayMessageKind::ToolResult, "TOOL_CALL_RESULT"),
        (DisplayMessageKind::ToolsUnavailable, "TOOLS_UNAVAILABLE"),
        (DisplayMessageKind::ToolSearchLoaded, "TOOL_SEARCH_LOADED"),
        (
            DisplayMessageKind::ToolSearchInitialized,
            "TOOL_SEARCH_INITIALIZED",
        ),
        (
            DisplayMessageKind::ToolSearchRefreshed,
            "TOOL_SEARCH_REFRESHED",
        ),
        (
            DisplayMessageKind::ToolSearchInvalidated,
            "TOOL_SEARCH_INVALIDATED",
        ),
        (DisplayMessageKind::ToolSearchFailed, "TOOL_SEARCH_FAILED"),
        (
            DisplayMessageKind::ToolSearchNoMatch,
            "TOOL_SEARCH_NO_MATCH",
        ),
        (
            DisplayMessageKind::ToolsetInitialized,
            "TOOLSET_INITIALIZED",
        ),
        (
            DisplayMessageKind::ToolsetUnavailable,
            "TOOLSET_UNAVAILABLE",
        ),
        (DisplayMessageKind::ToolsetFailed, "TOOLSET_FAILED"),
        (DisplayMessageKind::ToolsetRefreshed, "TOOLSET_REFRESHED"),
        (DisplayMessageKind::ToolsetClosed, "TOOLSET_CLOSED"),
        (DisplayMessageKind::ApprovalRequested, "APPROVAL_REQUESTED"),
        (DisplayMessageKind::ApprovalResolved, "APPROVAL_RESOLVED"),
        (DisplayMessageKind::HitlResolved, "HITL_RESOLVED"),
        (DisplayMessageKind::HitlDiagnostic, "HITL_DIAGNOSTIC"),
        (DisplayMessageKind::Checkpoint, "CHECKPOINT"),
        (DisplayMessageKind::SkillsScanned, "SKILLS_SCANNED"),
        (DisplayMessageKind::SkillActivated, "SKILL_ACTIVATED"),
        (DisplayMessageKind::SkillsReloaded, "SKILLS_RELOADED"),
        (DisplayMessageKind::SubagentStarted, "SUBAGENT_STARTED"),
        (DisplayMessageKind::SubagentCompleted, "SUBAGENT_COMPLETED"),
        (DisplayMessageKind::SubagentFailed, "SUBAGENT_FAILED"),
        (DisplayMessageKind::CompactionStarted, "COMPACTION_STARTED"),
        (
            DisplayMessageKind::CompactionCompleted,
            "COMPACTION_COMPLETED",
        ),
        (DisplayMessageKind::CompactionFailed, "COMPACTION_FAILED"),
        (DisplayMessageKind::HandoffStarted, "HANDOFF_STARTED"),
        (DisplayMessageKind::HandoffCompleted, "HANDOFF_COMPLETED"),
        (DisplayMessageKind::HandoffFailed, "HANDOFF_FAILED"),
        (DisplayMessageKind::SteeringSubmitted, "STEERING_SUBMITTED"),
        (DisplayMessageKind::SteeringReceived, "STEERING_RECEIVED"),
        (DisplayMessageKind::GoalIteration, "GOAL_ITERATION"),
        (DisplayMessageKind::GoalCompleted, "GOAL_COMPLETED"),
        (DisplayMessageKind::TaskSnapshot, "TASK_SNAPSHOT"),
        (DisplayMessageKind::TaskEvent, "TASK_EVENT"),
        (DisplayMessageKind::NoteEvent, "NOTE_EVENT"),
        (DisplayMessageKind::FileEvent, "FILE_EVENT"),
        (DisplayMessageKind::MediaEvent, "MEDIA_EVENT"),
        (DisplayMessageKind::HostEvent, "HOST_EVENT"),
        (DisplayMessageKind::RunCompleted, "RUN_FINISHED"),
        (DisplayMessageKind::RunFailed, "RUN_ERROR"),
        (DisplayMessageKind::RunCancelled, "RUN_CANCELLED"),
    ];

    for (kind, expected_type) in cases {
        let value = serde_json::to_value(display_message(1, kind, json!({}))).unwrap();
        assert_eq!(value["schema"], "starweaver.display.v1");
        assert_eq!(value["type"], expected_type);
        assert!(value.get("kind").is_none());
    }
}

fn sideband_display_context() -> DisplayProjectionContext {
    DisplayProjectionContext::new(
        SessionId::from_string("session-sideband"),
        RunId::from_string("run-sideband"),
    )
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn default_projector_maps_tool_sideband_custom_events() {
    let projector = DefaultDisplayMessageProjector;
    let context = sideband_display_context();
    let tools_unavailable = custom_stream_record(
        30,
        "tools_unavailable",
        &json!({"available": ["read"], "unavailable": ["write", "shell"]}),
    );
    let tool_search_loaded = custom_stream_record(
        31,
        "tool_search_loaded",
        &json!({"loaded_tools": ["lookup_docs"], "loaded_namespaces": ["docs"]}),
    );
    let tool_search_initialized = custom_stream_record(
        32,
        "tool_search_initialized",
        &json!({
            "total_tools": 12,
            "total_namespaces": 3,
            "available_tools": 10,
            "unavailable_tools": 2
        }),
    );
    let tool_search_refreshed = custom_stream_record(
        33,
        "tool_search_refreshed",
        &json!({"total_tools": 4, "total_namespaces": 0}),
    );
    let tool_search_invalidated = custom_stream_record(
        34,
        "tool_search_invalidated",
        &json!({
            "removed_loaded_tools": ["lookup_docs"],
            "removed_loaded_namespaces": ["docs"]
        }),
    );
    let tool_search_failed = custom_stream_record(
        35,
        "tool_search_failed",
        &json!({"message": "Parameter 'query' is required."}),
    );
    let tool_search_no_match =
        custom_stream_record(36, "tool_search_no_match", &json!({"query": "missing"}));
    let toolset_initialized = custom_stream_record(
        37,
        "toolset_initialized",
        &json!({"name": "docs", "state": "initialized", "tool_count": 2}),
    );
    let toolset_failed = custom_stream_record(
        38,
        "toolset_failed",
        &json!({"name": "remote", "state": "failed", "message": "offline"}),
    );

    let unavailable_messages = projector.project(&context, &tools_unavailable).await;
    let loaded_messages = projector.project(&context, &tool_search_loaded).await;
    let initialized_messages = projector.project(&context, &tool_search_initialized).await;
    let refreshed_messages = projector.project(&context, &tool_search_refreshed).await;
    let invalidated_messages = projector.project(&context, &tool_search_invalidated).await;
    let failed_messages = projector.project(&context, &tool_search_failed).await;
    let no_match_messages = projector.project(&context, &tool_search_no_match).await;
    let toolset_initialized_messages = projector.project(&context, &toolset_initialized).await;
    let toolset_failed_messages = projector.project(&context, &toolset_failed).await;

    assert_eq!(
        unavailable_messages[0].kind,
        DisplayMessageKind::ToolsUnavailable
    );
    assert_eq!(
        unavailable_messages[0].preview.as_deref(),
        Some("tools unavailable: 2 tool(s)")
    );
    assert_eq!(
        loaded_messages[0].kind,
        DisplayMessageKind::ToolSearchLoaded
    );
    assert_eq!(
        loaded_messages[0].preview.as_deref(),
        Some("tool search loaded 1 tool(s), 1 namespace(s)")
    );
    assert_eq!(
        initialized_messages[0].kind,
        DisplayMessageKind::ToolSearchInitialized
    );
    assert_eq!(
        initialized_messages[0].preview.as_deref(),
        Some("tool search initialized: 12 tool(s), 3 namespace(s)")
    );
    assert_eq!(
        refreshed_messages[0].kind,
        DisplayMessageKind::ToolSearchRefreshed
    );
    assert_eq!(
        refreshed_messages[0].preview.as_deref(),
        Some("tool search refreshed: 4 tool(s), 0 namespace(s)")
    );
    assert_eq!(
        invalidated_messages[0].kind,
        DisplayMessageKind::ToolSearchInvalidated
    );
    assert_eq!(
        invalidated_messages[0].preview.as_deref(),
        Some("tool search invalidated 1 tool(s), 1 namespace(s)")
    );
    assert_eq!(
        failed_messages[0].kind,
        DisplayMessageKind::ToolSearchFailed
    );
    assert_eq!(
        failed_messages[0].preview.as_deref(),
        Some("tool search failed: Parameter 'query' is required.")
    );
    assert_eq!(
        no_match_messages[0].kind,
        DisplayMessageKind::ToolSearchNoMatch
    );
    assert_eq!(
        no_match_messages[0].preview.as_deref(),
        Some("tool search no match: missing")
    );
    assert_eq!(
        toolset_initialized_messages[0].kind,
        DisplayMessageKind::ToolsetInitialized
    );
    assert_eq!(
        toolset_initialized_messages[0].preview.as_deref(),
        Some("docs initialized: 2 tool(s)")
    );
    assert_eq!(
        toolset_failed_messages[0].kind,
        DisplayMessageKind::ToolsetFailed
    );
    assert_eq!(
        toolset_failed_messages[0].preview.as_deref(),
        Some("remote failed: offline")
    );
}

#[tokio::test]
async fn default_projector_maps_skill_sideband_custom_events() {
    let projector = DefaultDisplayMessageProjector;
    let context = sideband_display_context();
    let skills_reloaded = custom_stream_record(
        34,
        "skills_reloaded",
        &json!({
            "package_count": 2,
            "diagnostics": [],
            "changes": [{"kind": "modified", "name": "research"}]
        }),
    );
    let skills_scanned = custom_stream_record(
        35,
        "skills_scanned",
        &json!({"package_count": 1, "diagnostics": [{"kind": "invalid_frontmatter"}]}),
    );
    let skill_activated = custom_stream_record(
        36,
        "skill_activated",
        &json!({"name": "research", "body_bytes": 42}),
    );

    let skill_messages = projector.project(&context, &skills_reloaded).await;
    let scanned_messages = projector.project(&context, &skills_scanned).await;
    let activated_messages = projector.project(&context, &skill_activated).await;

    assert_eq!(skill_messages[0].kind, DisplayMessageKind::SkillsReloaded);
    assert_eq!(
        skill_messages[0].preview.as_deref(),
        Some("skills reloaded: 2 package(s), 1 change(s), 0 diagnostic(s)")
    );
    assert_eq!(scanned_messages[0].kind, DisplayMessageKind::SkillsScanned);
    assert_eq!(
        scanned_messages[0].preview.as_deref(),
        Some("skills scanned: 1 package(s), 1 diagnostic(s)")
    );
    assert_eq!(
        activated_messages[0].kind,
        DisplayMessageKind::SkillActivated
    );
    assert_eq!(
        activated_messages[0].preview.as_deref(),
        Some("skill activated: research")
    );
}

#[tokio::test]
async fn default_projector_maps_hitl_sideband_custom_events() {
    let projector = DefaultDisplayMessageProjector;
    let context = sideband_display_context();
    let approval_requested =
        custom_stream_record(37, "approval_requested", &json!({"tool_name": "edit"}));
    let approval_resolved =
        custom_stream_record(38, "approval_resolved", &json!({"status": "approved"}));
    let hitl_resolved = custom_stream_record(
        39,
        "hitl_resolved",
        &json!({
            "tool_returns": 3,
            "approved": 1,
            "denied": 1,
            "deferred_completed": 1,
            "deferred_failed": 0,
            "deferred_cancelled": 0
        }),
    );
    let hitl_diagnostic = custom_stream_record(
        40,
        "hitl_decision_diagnostic",
        &json!({"error_kind": "duplicate_decision", "decision_id": "call_duplicate"}),
    );

    let approval_requested_messages = projector.project(&context, &approval_requested).await;
    let approval_resolved_messages = projector.project(&context, &approval_resolved).await;
    let hitl_messages = projector.project(&context, &hitl_resolved).await;
    let hitl_diagnostic_messages = projector.project(&context, &hitl_diagnostic).await;

    assert_eq!(
        approval_requested_messages[0].kind,
        DisplayMessageKind::ApprovalRequested
    );
    assert_eq!(
        approval_requested_messages[0].preview.as_deref(),
        Some("approval requested: edit")
    );
    assert_eq!(
        approval_resolved_messages[0].kind,
        DisplayMessageKind::ApprovalResolved
    );
    assert_eq!(
        approval_resolved_messages[0].preview.as_deref(),
        Some("approval resolved: approved")
    );
    assert_eq!(hitl_messages[0].kind, DisplayMessageKind::HitlResolved);
    assert_eq!(
        hitl_messages[0].preview.as_deref(),
        Some(
            "hitl resolved: 3 return(s), 1 approved, 1 denied, 1 deferred completed, 0 deferred failed, 0 deferred cancelled"
        )
    );
    assert_eq!(
        hitl_diagnostic_messages[0].kind,
        DisplayMessageKind::HitlDiagnostic
    );
    assert_eq!(
        hitl_diagnostic_messages[0].preview.as_deref(),
        Some("hitl diagnostic: duplicate_decision: call_duplicate")
    );
}

#[tokio::test]
async fn default_projector_maps_subagent_sideband_custom_events() {
    let projector = DefaultDisplayMessageProjector;
    let context = sideband_display_context();
    let subagent_started = custom_stream_record(
        40,
        "subagent_started",
        &json!({"name": "research", "task_id": "research-1"}),
    );
    let subagent_completed = custom_stream_record(
        41,
        "subagent_completed",
        &json!({"name": "research", "task_id": "research-1"}),
    );
    let subagent_failed = custom_stream_record(
        42,
        "subagent_failed",
        &json!({"name": "research", "metadata": {"error": "missing_subagent"}}),
    );

    let subagent_started_messages = projector.project(&context, &subagent_started).await;
    let subagent_completed_messages = projector.project(&context, &subagent_completed).await;
    let subagent_messages = projector.project(&context, &subagent_failed).await;

    assert_eq!(
        subagent_started_messages[0].kind,
        DisplayMessageKind::SubagentStarted
    );
    assert_eq!(
        subagent_started_messages[0].preview.as_deref(),
        Some("subagent started: research")
    );
    assert_eq!(
        subagent_completed_messages[0].kind,
        DisplayMessageKind::SubagentCompleted
    );
    assert_eq!(
        subagent_completed_messages[0].preview.as_deref(),
        Some("subagent completed: research")
    );
    assert_eq!(
        subagent_messages[0].kind,
        DisplayMessageKind::SubagentFailed
    );
    assert_eq!(
        subagent_messages[0].preview.as_deref(),
        Some("subagent failed: research: missing_subagent")
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn steering_projection_shows_main_and_hides_all_subagent_steering_in_replay() {
    let projector = DefaultDisplayMessageProjector;
    let context = sideband_display_context();
    let main = custom_stream_record(
        20,
        "steering_received",
        &json!({"id": "steer-main", "text": "keep the main path"}),
    );
    let source = AgentStreamSource::subagent(
        AgentId::from_string("child-agent"),
        "child",
        TaskId::from_string("child-task"),
        Some(RunId::from_string("run-child")),
        Some(context.run_id.clone()),
        4,
    );
    let subagent = custom_stream_record(
        21,
        "steering_received",
        &json!({"id": "steer-child", "text": "private child update"}),
    )
    .with_source(source.clone());
    let subagent_submitted = custom_stream_record(
        22,
        "steering_submitted",
        &json!({"id": "steer-child", "text": "private child submission"}),
    )
    .with_source(source.clone());
    let subagent_guard = AgentStreamRecord::new(
        23,
        AgentStreamEvent::SteeringGuard {
            step: 2,
            prompt: "private child guard".to_string(),
        },
    )
    .with_source(source.clone());
    let namespaced_subagent = custom_stream_record(
        24,
        "runtime.steer_ack",
        &json!({"id": "steer-child", "text": "private child acknowledgement"}),
    )
    .with_source(source.clone());
    let unrelated_subagent = custom_stream_record(
        25,
        "task_steer_progress",
        &json!({"text": "not a steering protocol event"}),
    )
    .with_source(source);

    assert!(!main.is_subagent_steering_event());
    assert!(subagent.is_subagent_steering_event());
    assert!(subagent_submitted.is_subagent_steering_event());
    assert!(subagent_guard.is_subagent_steering_event());
    assert!(namespaced_subagent.is_subagent_steering_event());
    assert!(!unrelated_subagent.is_subagent_steering_event());
    let main_live = projector.project(&context, &main).await;
    let subagent_live = projector.project(&context, &subagent).await;
    let namespaced_subagent_live = projector.project(&context, &namespaced_subagent).await;
    assert_eq!(main_live.len(), 1);
    assert_eq!(main_live[0].kind, DisplayMessageKind::SteeringReceived);
    assert_eq!(main_live[0].payload["text"], "keep the main path");
    assert!(subagent_live.is_empty());
    assert!(namespaced_subagent_live.is_empty());

    let archive = InMemoryStreamArchive::new();
    archive
        .append_raw_records(
            &context.session_id,
            &context.run_id,
            vec![
                main.clone(),
                subagent.clone(),
                subagent_submitted.clone(),
                subagent_guard.clone(),
            ],
        )
        .await
        .unwrap();
    let projected = projector.project_records(
        &context,
        &[
            main.clone(),
            subagent.clone(),
            subagent_submitted.clone(),
            subagent_guard.clone(),
        ],
    );
    assert_eq!(projected.len(), 1);
    assert_eq!(projected[0].kind, DisplayMessageKind::SteeringReceived);
    assert_eq!(projected[0].payload, main_live[0].payload);
    let scope = ReplayScope::run(context.run_id.as_str());
    archive
        .append_display_messages(scope.clone(), projected.clone())
        .await
        .unwrap();

    let raw_replay = archive
        .replay_raw_after(&context.session_id, &context.run_id, None)
        .await
        .unwrap();
    assert_eq!(
        raw_replay,
        vec![main, subagent, subagent_submitted, subagent_guard]
    );
    let display_replay = archive.replay_display_after(&scope, None).await.unwrap();
    assert_eq!(display_replay, projected);
}

#[tokio::test]
async fn default_projector_maps_generic_sideband_custom_events() {
    let projector = DefaultDisplayMessageProjector;
    let context = sideband_display_context();
    let task_updated = custom_stream_record(
        43,
        "task_updated",
        &json!({"subject": "Ship display events", "status": "completed"}),
    );
    let note_set = custom_stream_record(44, "note_set", &json!({"name": "design"}));
    let file_changed = custom_stream_record(45, "file_changed", &json!({"path": "src/lib.rs"}));
    let media_uploaded = custom_stream_record(
        46,
        "media_uploaded",
        &json!({"uri": "resource://uploaded/image"}),
    );
    let host_event = custom_stream_record(
        47,
        "host_browser_opened",
        &json!({"operation": "browser_opened"}),
    );

    let task_messages = projector.project(&context, &task_updated).await;
    let note_messages = projector.project(&context, &note_set).await;
    let file_messages = projector.project(&context, &file_changed).await;
    let media_messages = projector.project(&context, &media_uploaded).await;
    let host_messages = projector.project(&context, &host_event).await;

    assert_eq!(task_messages[0].kind, DisplayMessageKind::TaskEvent);
    assert_eq!(
        task_messages[0].preview.as_deref(),
        Some("task event: Ship display events")
    );
    assert_eq!(
        task_messages[0].metadata["starweaver_event_kind"],
        json!("task_updated")
    );
    assert_eq!(note_messages[0].kind, DisplayMessageKind::NoteEvent);
    assert_eq!(
        note_messages[0].preview.as_deref(),
        Some("note event: design")
    );
    assert_eq!(file_messages[0].kind, DisplayMessageKind::FileEvent);
    assert_eq!(
        file_messages[0].preview.as_deref(),
        Some("file event: src/lib.rs")
    );
    assert_eq!(media_messages[0].kind, DisplayMessageKind::MediaEvent);
    assert_eq!(
        media_messages[0].preview.as_deref(),
        Some("media event: resource://uploaded/image")
    );
    assert_eq!(host_messages[0].kind, DisplayMessageKind::HostEvent);
    assert_eq!(
        host_messages[0].preview.as_deref(),
        Some("host event: browser_opened")
    );
}

#[tokio::test]
async fn replay_log_orders_replays_after_cursor_and_is_idempotent() {
    let log = InMemoryReplayEventLog::new();
    let scope = ReplayScope::run("run-1");
    let event_two = ReplayEvent::new(scope.clone(), 2, ReplayEventKind::Raw(json!({"n": 2})));
    let event_one = ReplayEvent::new(scope.clone(), 1, ReplayEventKind::Raw(json!({"n": 1})));
    log.append(scope.clone(), event_two.clone()).await.unwrap();
    log.append(scope.clone(), event_one.clone()).await.unwrap();
    log.append(scope.clone(), event_two.clone()).await.unwrap();
    let conflict = ReplayEvent {
        event: ReplayEventKind::Raw(json!({"n": 99})),
        ..event_two
    };
    let error = log.append(scope.clone(), conflict).await.unwrap_err();
    assert!(error.to_string().contains("replay event conflict"));

    let replay = log
        .replay_after(
            &scope,
            Some(ReplayCursor::replay_event(scope.clone(), 1)),
            None,
        )
        .await
        .unwrap();
    assert_eq!(replay.len(), 1);
    assert_eq!(replay[0].sequence, 2);
}

#[tokio::test]
async fn replay_subscription_receives_live_tail_after_cursor() {
    let log = InMemoryReplayEventLog::new();
    let scope = ReplayScope::run("run-live");
    log.append(
        scope.clone(),
        ReplayEvent::new(scope.clone(), 1, ReplayEventKind::Heartbeat),
    )
    .await
    .unwrap();
    let mut subscription = log
        .subscribe(
            scope.clone(),
            Some(ReplayCursor::replay_event(scope.clone(), 1)),
        )
        .await
        .unwrap();
    log.append(
        scope.clone(),
        ReplayEvent::new(
            scope.clone(),
            2,
            ReplayEventKind::Terminal {
                marker: StreamTerminalMarker::RunCompleted,
            },
        ),
    )
    .await
    .unwrap();

    let event = subscription.recv().await.unwrap();
    assert_eq!(event.sequence, 2);
    assert!(matches!(
        event.event,
        ReplayEventKind::Terminal {
            marker: StreamTerminalMarker::RunCompleted
        }
    ));
}

#[test]
fn realtime_compaction_merges_text_and_tool_deltas_and_retains_terminal() {
    let scope = ReplayScope::run("run-compact");
    let mut buffer = RealtimeCompactionBuffer::new(scope.clone());
    buffer.push(display_message(
        1,
        DisplayMessageKind::AssistantTextDelta,
        json!({"part_index": 0, "delta": "hel"}),
    ));
    buffer.push(display_message(
        2,
        DisplayMessageKind::AssistantTextDelta,
        json!({"part_index": 0, "delta": "lo"}),
    ));
    buffer.push(display_message(
        3,
        DisplayMessageKind::ToolCallDelta,
        json!({"tool_call_id": "call-1", "tool_name": "lookup", "delta": "{\"q\""}),
    ));
    buffer.push(display_message(
        4,
        DisplayMessageKind::ToolCallDelta,
        json!({"tool_call_id": "call-1", "delta": ":\"x\"}"}),
    ));
    buffer.push(display_message(
        5,
        DisplayMessageKind::RunCompleted,
        json!({"output": "done"}),
    ));

    let snapshot = buffer.snapshot();
    assert_eq!(snapshot.scope, Some(scope));
    assert_eq!(snapshot.display_messages.len(), 3);
    assert_eq!(snapshot.display_messages[0].payload["text"], "hello");
    assert_eq!(
        snapshot.display_messages[1].payload["arguments_delta"],
        "{\"q\":\"x\"}"
    );
    assert_eq!(snapshot.display_messages[1].payload["tool_name"], "lookup");
    assert_eq!(
        snapshot.display_messages[2].kind,
        DisplayMessageKind::RunCompleted
    );
    assert_eq!(
        buffer
            .tail_after(Some(ReplayCursor::display(
                ReplayScope::run("run-compact"),
                4,
            )))
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn stream_archive_replays_raw_display_snapshots_and_cursor_range() {
    let archive = InMemoryStreamArchive::new();
    let session_id = SessionId::from_string("session-a");
    let run_id = RunId::from_string("run-a");
    let scope = ReplayScope::run(run_id.as_str());
    archive
        .append_raw_records(
            &session_id,
            &run_id,
            vec![
                AgentStreamRecord::new(0, AgentStreamEvent::ModelRequest { step: 0 }),
                AgentStreamRecord::new(1, AgentStreamEvent::ModelRequest { step: 1 }),
            ],
        )
        .await
        .unwrap();
    let error = archive
        .append_raw_records(
            &session_id,
            &run_id,
            vec![
                AgentStreamRecord::new(2, AgentStreamEvent::ModelRequest { step: 2 }),
                AgentStreamRecord::new(1, AgentStreamEvent::ModelRequest { step: 99 }),
            ],
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("raw stream conflict"));
    archive
        .append_display_messages(
            scope.clone(),
            vec![
                display_message(0, DisplayMessageKind::RunStarted, json!({})),
                display_message(1, DisplayMessageKind::RunCompleted, json!({"output":"ok"})),
            ],
        )
        .await
        .unwrap();
    let snapshot = ReplaySnapshot {
        scope: Some(scope.clone()),
        revision: 1,
        cursor: Some(ReplayCursor::display(scope.clone(), 1)),
        display_messages: vec![display_message(
            1,
            DisplayMessageKind::RunCompleted,
            json!({}),
        )],
        metadata: starweaver_core::Metadata::default(),
    };
    archive
        .append_snapshot(scope.clone(), snapshot.clone())
        .await
        .unwrap();

    let raw = archive
        .replay_raw_after(
            &session_id,
            &run_id,
            Some(ReplayCursor::raw_runtime(scope.clone(), 0)),
        )
        .await
        .unwrap();
    let display = archive
        .replay_display_after(&scope, Some(ReplayCursor::display(scope.clone(), 0)))
        .await
        .unwrap();
    let range = archive.cursor_range(&scope).await.unwrap().unwrap();
    assert_eq!(raw.len(), 1);
    assert_eq!(display.len(), 1);
    assert_eq!(
        archive.latest_snapshot(&scope).await.unwrap(),
        Some(snapshot)
    );
    assert_eq!(range.0.sequence, 0);
    assert_eq!(range.1.sequence, 1);
}

#[tokio::test]
async fn transport_builds_sse_and_jsonl_envelopes() {
    let log = InMemoryReplayEventLog::new();
    let scope = ReplayScope::run("run-transport");
    log.append(
        scope.clone(),
        ReplayEvent::new(
            scope.clone(),
            1,
            ReplayEventKind::Raw(json!({"hello":"world"})),
        ),
    )
    .await
    .unwrap();

    let sse = InMemoryReplayTransport::sse(log.clone())
        .replay(scope.clone(), None)
        .await
        .unwrap();
    let jsonl = InMemoryReplayTransport::jsonl(log)
        .replay(scope.clone(), None)
        .await
        .unwrap();

    match &sse[0] {
        ReplayEnvelope::Sse(envelope) => {
            assert_eq!(envelope.id, "1");
            assert!(envelope.to_frame().contains("event: raw"));
        }
        ReplayEnvelope::Jsonl(_) => panic!("expected sse"),
    }
    match &jsonl[0] {
        ReplayEnvelope::Jsonl(envelope) => {
            assert_eq!(envelope.sequence, 1);
            assert!(envelope.to_line().unwrap().contains("hello"));
        }
        ReplayEnvelope::Sse(_) => panic!("expected jsonl"),
    }
}

#[tokio::test]
async fn default_projector_preserves_source_attribution_on_nested_records() {
    let projector = DefaultDisplayMessageProjector;
    let context = DisplayProjectionContext::new(
        SessionId::from_string("session-source"),
        RunId::from_string("run-parent"),
    );
    let record = AgentStreamRecord::new(
        7,
        AgentStreamEvent::ModelStream {
            step: 0,
            event: ModelResponseStreamEvent::PartDelta(PartDelta::text(0, "child delta")),
        },
    )
    .with_source(AgentStreamSource::subagent(
        AgentId::from_string("researcher-1"),
        "researcher",
        TaskId::from_string("task-research"),
        Some(RunId::from_string("run-child")),
        Some(RunId::from_string("run-parent")),
        3,
    ));

    let messages = projector.project(&context, &record).await;

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].kind, DisplayMessageKind::AssistantTextDelta);
    assert_eq!(messages[0].run_id.as_str(), "run-child");
    assert_eq!(
        messages[0]
            .agent_id
            .as_ref()
            .map(starweaver_core::AgentId::as_str),
        Some("researcher-1")
    );
    assert_eq!(messages[0].agent_name.as_deref(), Some("researcher"));
    assert_eq!(messages[0].metadata["source_kind"], json!("subagent"));
    assert_eq!(
        messages[0].metadata["source_agent_id"],
        json!("researcher-1")
    );
    assert_eq!(
        messages[0].metadata["source_agent_name"],
        json!("researcher")
    );
    assert_eq!(
        messages[0].metadata["source_task_id"],
        json!("task-research")
    );
    assert_eq!(messages[0].metadata["source_sequence"], json!(3));
}

#[tokio::test]
async fn default_projector_maps_runtime_stream_parts_to_display_messages() {
    let projector = DefaultDisplayMessageProjector;
    let session_id = SessionId::from_string("session-project");
    let run_id = RunId::from_string("run-project");
    let context = DisplayProjectionContext::new(session_id, run_id.clone());
    let start = AgentStreamRecord::new(
        0,
        AgentStreamEvent::RunStart {
            run_id: run_id.clone(),
            conversation_id: starweaver_core::ConversationId::from_string("conv-project"),
        },
    );
    let delta = AgentStreamRecord::new(
        1,
        AgentStreamEvent::ModelStream {
            step: 0,
            event: ModelResponseStreamEvent::PartDelta(PartDelta::text(0, "hi")),
        },
    );
    let part_start = AgentStreamRecord::new(
        2,
        AgentStreamEvent::ModelStream {
            step: 0,
            event: ModelResponseStreamEvent::PartStart(PartStart {
                index: 0,
                part_kind: "text".to_string(),
            }),
        },
    );
    let part_end = AgentStreamRecord::new(
        3,
        AgentStreamEvent::ModelStream {
            step: 0,
            event: ModelResponseStreamEvent::PartEnd(PartEnd {
                index: 0,
                part_kind: Some("text".to_string()),
            }),
        },
    );

    let start_messages = projector.project(&context, &start).await;
    let delta_messages = projector.project(&context, &delta).await;
    let part_start_messages = projector.project(&context, &part_start).await;
    let part_end_messages = projector.project(&context, &part_end).await;

    assert_eq!(start_messages[0].kind, DisplayMessageKind::RunStarted);
    assert_eq!(start_messages[0].run_id, run_id);
    assert_eq!(
        delta_messages[0].kind,
        DisplayMessageKind::AssistantTextDelta
    );
    assert_eq!(delta_messages[0].payload["delta"], "hi");
    assert_eq!(delta_messages[0].run_id, run_id);
    assert_eq!(
        part_start_messages[0].kind,
        DisplayMessageKind::AssistantTextStart
    );
    assert_eq!(
        part_end_messages[0].kind,
        DisplayMessageKind::AssistantTextEnd
    );
}

#[tokio::test]
async fn default_projector_maps_summary_and_compaction_custom_events() {
    let projector = DefaultDisplayMessageProjector;
    let context = DisplayProjectionContext::new(
        SessionId::from_string("session-context-events"),
        RunId::from_string("run-context-events"),
    );

    let compact_started = custom_stream_record(
        10,
        "starweaver.compaction_started",
        &json!({"message_count": 50}),
    );
    let handoff_completed =
        custom_stream_record(11, "summary_complete", &json!({"content": "handoff body"}));
    let steering_received = custom_stream_record(
        12,
        "steering_received",
        &json!({"id": "steer_0", "text": "keep going"}),
    );
    let goal_iteration = custom_stream_record(
        13,
        "goal_iteration",
        &json!({"iteration": 1, "max_iterations": 10, "task": "ship"}),
    );
    let goal_complete = custom_stream_record(
        14,
        "goal_complete",
        &json!({"iteration": 1, "max_iterations": 10, "reason": "verified", "task": "ship"}),
    );
    let task_snapshot = custom_stream_record(
        15,
        "task_snapshot",
        &json!({"tasks": [{"id": "1", "subject": "Ship", "status": "pending"}]}),
    );
    let unrelated = custom_stream_record(16, "unknown_event", &json!({"ok": true}));

    let compact_messages = projector.project(&context, &compact_started).await;
    let handoff_messages = projector.project(&context, &handoff_completed).await;
    let steering_messages = projector.project(&context, &steering_received).await;
    let goal_iteration_messages = projector.project(&context, &goal_iteration).await;
    let goal_complete_messages = projector.project(&context, &goal_complete).await;
    let task_messages = projector.project(&context, &task_snapshot).await;
    let unrelated_messages = projector.project(&context, &unrelated).await;

    assert_eq!(compact_messages.len(), 1);
    assert_eq!(
        compact_messages[0].kind,
        DisplayMessageKind::CompactionStarted
    );
    assert_eq!(compact_messages[0].payload["message_count"], 50);
    assert_eq!(
        compact_messages[0].preview.as_deref(),
        Some("context compacting 50 messages")
    );
    assert_eq!(handoff_messages.len(), 1);
    assert_eq!(
        handoff_messages[0].kind,
        DisplayMessageKind::HandoffCompleted
    );
    assert_eq!(handoff_messages[0].payload["content"], "handoff body");
    assert_eq!(steering_messages.len(), 1);
    assert_eq!(
        steering_messages[0].kind,
        DisplayMessageKind::SteeringReceived
    );
    assert_eq!(steering_messages[0].payload["text"], "keep going");
    assert_eq!(
        steering_messages[0].preview.as_deref(),
        Some("steering received: keep going")
    );
    assert_eq!(goal_iteration_messages.len(), 1);
    assert_eq!(
        goal_iteration_messages[0].kind,
        DisplayMessageKind::GoalIteration
    );
    assert_eq!(
        goal_iteration_messages[0].preview.as_deref(),
        Some("goal iteration 1/10")
    );
    assert_eq!(goal_complete_messages.len(), 1);
    assert_eq!(
        goal_complete_messages[0].kind,
        DisplayMessageKind::GoalCompleted
    );
    assert_eq!(
        goal_complete_messages[0].preview.as_deref(),
        Some("goal completed: verified")
    );
    assert_eq!(task_messages.len(), 1);
    assert_eq!(task_messages[0].kind, DisplayMessageKind::TaskSnapshot);
    assert_eq!(task_messages[0].payload["tasks"][0]["subject"], "Ship");
    assert_eq!(
        task_messages[0].preview.as_deref(),
        Some("task snapshot: 1 task(s)")
    );
    assert!(unrelated_messages.is_empty());
}

#[tokio::test]
async fn default_projector_maps_summarize_tool_to_handoff_events() {
    let projector = DefaultDisplayMessageProjector;
    let context = DisplayProjectionContext::new(
        SessionId::from_string("session-summarize"),
        RunId::from_string("run-summarize"),
    );
    let call = ToolCallPart {
        id: "summarize-call".to_string(),
        name: "summarize".to_string(),
        arguments: json!({"content": "handoff body"}).into(),
    };
    let call_record = AgentStreamRecord::new(20, AgentStreamEvent::ToolCall { step: 1, call });
    let return_record = AgentStreamRecord::new(
        21,
        AgentStreamEvent::ToolReturn {
            step: 1,
            tool_return: ToolReturnPart::new(
                "summarize-call",
                "summarize",
                json!({
                    "operation": "summarize",
                    "payload": {
                        "content": "handoff body",
                        "auto_load_files": ["AGENTS.md"]
                    }
                }),
            ),
        },
    );

    let call_messages = projector.project(&context, &call_record).await;
    let return_messages = projector.project(&context, &return_record).await;

    assert_eq!(call_messages[0].kind, DisplayMessageKind::HandoffStarted);
    assert_eq!(call_messages[0].payload["content"], "handoff body");
    assert_eq!(return_messages.len(), 2);
    assert_eq!(return_messages[0].kind, DisplayMessageKind::ToolResult);
    assert_eq!(
        return_messages[1].kind,
        DisplayMessageKind::HandoffCompleted
    );
    assert_eq!(return_messages[1].payload["content"], "handoff body");
    assert_eq!(
        return_messages[1].payload["auto_load_files"][0],
        "AGENTS.md"
    );
}

#[tokio::test]
async fn default_projector_maps_final_results_and_prefers_context_run_id() {
    let projector = DefaultDisplayMessageProjector;
    let session_id = SessionId::from_string("session-project-final");
    let run_id = RunId::from_string("run-project-final");
    let context = DisplayProjectionContext::new(session_id, run_id.clone());
    let model_response = AgentStreamRecord::new(
        4,
        AgentStreamEvent::ModelResponse {
            step: 1,
            response: ModelResponse::text("done"),
        },
    );
    let final_result = AgentStreamRecord::new(
        5,
        AgentStreamEvent::ModelStream {
            step: 1,
            event: ModelResponseStreamEvent::FinalResult(Box::new(ModelResponse::text("done"))),
        },
    );
    let runtime_start = AgentStreamRecord::new(
        6,
        AgentStreamEvent::RunStart {
            run_id: RunId::from_string("runtime-run"),
            conversation_id: starweaver_core::ConversationId::from_string("conv-runtime"),
        },
    );

    let model_response_messages = projector.project(&context, &model_response).await;
    let final_result_messages = projector.project(&context, &final_result).await;
    let runtime_start_messages = projector.project(&context, &runtime_start).await;

    assert_eq!(model_response_messages.len(), 3);
    assert_eq!(
        model_response_messages[0].kind,
        DisplayMessageKind::AssistantTextStart
    );
    assert_eq!(
        model_response_messages[1].kind,
        DisplayMessageKind::AssistantTextDelta
    );
    assert_eq!(model_response_messages[1].payload["delta"], "done");
    assert_eq!(
        model_response_messages[2].kind,
        DisplayMessageKind::AssistantTextEnd
    );
    assert_eq!(final_result_messages.len(), 3);
    assert_eq!(
        final_result_messages[1].kind,
        DisplayMessageKind::AssistantTextDelta
    );
    assert_eq!(final_result_messages[1].payload["delta"], "done");
    assert_eq!(runtime_start_messages[0].run_id, run_id);
}

#[tokio::test]
async fn default_projector_maps_thinking_and_tool_calls_from_model_response() {
    let projector = DefaultDisplayMessageProjector;
    let session_id = SessionId::from_string("session-project-parts");
    let run_id = RunId::from_string("run-project-parts");
    let context = DisplayProjectionContext::new(session_id, run_id.clone());
    let response = ModelResponse {
        parts: vec![
            ModelResponsePart::ProviderThinking {
                text: "inspect context".to_string(),
                signature: Some("sig".to_string()),
                provider: ProviderPartInfo::new("openai").with_id("rs_1"),
            },
            ModelResponsePart::ProviderToolCall {
                call: ToolCallPart {
                    id: "call_1".to_string(),
                    name: "lookup".to_string(),
                    arguments: json!({"query": "starweaver"}).into(),
                },
                provider: ProviderPartInfo::new("openai").with_id("fc_1"),
            },
            ModelResponsePart::ProviderText {
                text: "done".to_string(),
                provider: ProviderPartInfo::new("openai").with_id("msg_1"),
            },
        ],
        usage: starweaver_usage::Usage::default(),
        model_name: None,
        provider: None,
        finish_reason: None,
        timestamp: None,
        run_id: None,
        conversation_id: None,
        metadata: starweaver_core::Metadata::default(),
    };
    let record = AgentStreamRecord::new(9, AgentStreamEvent::ModelResponse { step: 1, response });

    let messages = projector.project(&context, &record).await;

    let thinking_messages = messages
        .iter()
        .filter(|message| message.payload["part_kind"] == "thinking")
        .collect::<Vec<_>>();
    assert!(thinking_messages.iter().any(|message| {
        message.kind == DisplayMessageKind::AssistantTextDelta
            && message.payload["delta"] == "inspect context"
    }));
    assert!(
        thinking_messages
            .iter()
            .any(|message| message.payload["has_signature"] == true)
    );
    assert!(
        thinking_messages
            .iter()
            .all(|message| message.payload.get("signature").is_none())
    );
    assert!(
        !serde_json::to_string(&thinking_messages)
            .unwrap()
            .contains("\"sig\"")
    );
    assert!(messages.iter().any(|message| {
        message.kind == DisplayMessageKind::ToolCallStart
            && message.payload["tool_name"] == "lookup"
    }));
    assert!(messages.iter().any(|message| {
        message.kind == DisplayMessageKind::ToolCallDelta
            && message.payload["tool_name"] == "lookup"
            && message.payload["arguments"] == json!({"query": "starweaver"})
            && message.payload["delta"] == r#"{"query":"starweaver"}"#
    }));
    assert!(messages.iter().any(|message| {
        message.kind == DisplayMessageKind::ToolCallEnd
            && message.payload["tool_call_id"] == "call_1"
    }));
    assert!(messages.iter().any(|message| {
        message.kind == DisplayMessageKind::AssistantTextDelta && message.payload["delta"] == "done"
    }));
}

#[tokio::test]
async fn replay_subscription_refills_broadcast_lag_from_durable_events() {
    let log = InMemoryReplayEventLog::new();
    let scope = ReplayScope::run("lag-refill");
    let mut subscription = log.subscribe(scope.clone(), None).await.unwrap();
    for sequence in 0..300 {
        log.append(
            scope.clone(),
            ReplayEvent::new(scope.clone(), sequence, ReplayEventKind::Heartbeat),
        )
        .await
        .unwrap();
    }

    for expected in 0..300 {
        let event = subscription.recv().await.unwrap();
        assert_eq!(event.sequence, expected);
    }
}

#[tokio::test]
async fn replay_subscription_orders_concurrent_out_of_order_live_publication() {
    let log = InMemoryReplayEventLog::new();
    let scope = ReplayScope::run("concurrent-refill");
    let mut subscription = log.subscribe(scope.clone(), None).await.unwrap();
    let high = log.append(
        scope.clone(),
        ReplayEvent::new(scope.clone(), 1, ReplayEventKind::Heartbeat),
    );
    let low = log.append(
        scope.clone(),
        ReplayEvent::new(scope.clone(), 0, ReplayEventKind::Heartbeat),
    );
    let (high_result, low_result) = tokio::join!(high, low);
    high_result.unwrap();
    low_result.unwrap();

    assert_eq!(subscription.recv().await.unwrap().sequence, 0);
    assert_eq!(subscription.recv().await.unwrap().sequence, 1);
}
