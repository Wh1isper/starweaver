#![allow(clippy::unwrap_used)]

use serde_json::json;
use starweaver_core::RunId;
use starweaver_model::{
    ModelResponse, ModelResponsePart, ModelResponseStreamEvent, PartDelta, PartEnd, PartStart,
    ProviderPartInfo, ToolCallPart, ToolReturnPart,
};
use starweaver_runtime::{AgentStreamEvent, AgentStreamRecord};

use super::*;
use starweaver_core::SessionId;

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
        (DisplayMessageKind::HostOperation, "HOST_OPERATION"),
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
        Some("hitl resolved: 3 return(s), 1 approved, 1 denied, 1 deferred completed, 0 deferred failed, 0 deferred cancelled")
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
    let host_operation = custom_stream_record(
        47,
        "host_browser_opened",
        &json!({"operation": "browser_opened"}),
    );

    let task_messages = projector.project(&context, &task_updated).await;
    let note_messages = projector.project(&context, &note_set).await;
    let file_messages = projector.project(&context, &file_changed).await;
    let media_messages = projector.project(&context, &media_uploaded).await;
    let host_messages = projector.project(&context, &host_operation).await;

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
    assert_eq!(host_messages[0].kind, DisplayMessageKind::HostOperation);
    assert_eq!(
        host_messages[0].preview.as_deref(),
        Some("host operation: browser_opened")
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
    log.append(scope.clone(), event_two).await.unwrap();

    let replay = log
        .replay_after(&scope, Some(ReplayCursor::new(scope.clone(), 1)), None)
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
        .subscribe(scope.clone(), Some(ReplayCursor::new(scope.clone(), 1)))
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
            .tail_after(Some(ReplayCursor::new(ReplayScope::run("run-compact"), 4)))
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
        cursor: Some(ReplayCursor::new(scope.clone(), 1)),
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
            Some(ReplayCursor::new(scope.clone(), 0)),
        )
        .await
        .unwrap();
    let display = archive
        .replay_display_after(&scope, Some(ReplayCursor::new(scope.clone(), 0)))
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
    assert!(thinking_messages
        .iter()
        .any(|message| message.payload["has_signature"] == true));
    assert!(thinking_messages
        .iter()
        .all(|message| message.payload.get("signature").is_none()));
    assert!(!serde_json::to_string(&thinking_messages)
        .unwrap()
        .contains("\"sig\""));
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
