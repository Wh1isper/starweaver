#![allow(clippy::unwrap_used)]

use serde_json::json;
use starweaver_core::RunId;
use starweaver_model::{
    ModelResponse, ModelResponsePart, ModelResponseStreamEvent, PartDelta, PartEnd, PartStart,
    ToolCallPart, ToolReturnPart,
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
fn display_message_serializes_as_agui_compatible_event() {
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
fn all_display_message_kinds_serialize_to_agui_compatible_types() {
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
        (DisplayMessageKind::ApprovalRequested, "APPROVAL_REQUESTED"),
        (DisplayMessageKind::ApprovalResolved, "APPROVAL_RESOLVED"),
        (DisplayMessageKind::Checkpoint, "CHECKPOINT"),
        (DisplayMessageKind::SubagentStarted, "SUBAGENT_STARTED"),
        (DisplayMessageKind::SubagentCompleted, "SUBAGENT_COMPLETED"),
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

#[test]
fn display_message_accepts_legacy_snake_case_kind_alias() {
    let mut value = serde_json::to_value(display_message(
        1,
        DisplayMessageKind::AssistantTextDelta,
        json!({"delta": "hello"}),
    ))
    .unwrap();
    value["kind"] = json!("assistant_text_delta");
    value.as_object_mut().unwrap().remove("type");
    let message = serde_json::from_value::<DisplayMessage>(value).unwrap();
    assert_eq!(message.kind, DisplayMessageKind::AssistantTextDelta);
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
            ReplayEventKind::Terminal(StreamTerminalMarker::RunCompleted),
        ),
    )
    .await
    .unwrap();

    let event = subscription.recv().await.unwrap();
    assert_eq!(event.sequence, 2);
    assert!(matches!(
        event.event,
        ReplayEventKind::Terminal(StreamTerminalMarker::RunCompleted)
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
    let unrelated = custom_stream_record(13, "task_panel", &json!({"tasks": []}));

    let compact_messages = projector.project(&context, &compact_started).await;
    let handoff_messages = projector.project(&context, &handoff_completed).await;
    let steering_messages = projector.project(&context, &steering_received).await;
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
            ModelResponsePart::Thinking {
                text: "inspect context".to_string(),
                signature: Some("sig".to_string()),
            },
            ModelResponsePart::ToolCall(ToolCallPart {
                id: "call_1".to_string(),
                name: "lookup".to_string(),
                arguments: json!({"query": "starweaver"}).into(),
            }),
            ModelResponsePart::Text {
                text: "done".to_string(),
            },
        ],
        usage: starweaver_core::Usage::default(),
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

    assert!(messages.iter().any(|message| {
        message.kind == DisplayMessageKind::AssistantTextDelta
            && message.payload["part_kind"] == "thinking"
            && message.payload["delta"] == "inspect context"
    }));
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
