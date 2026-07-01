//! SSE replay transport tests.

use serde_json::json;
use starweaver_stream::{
    ReplayEvent, ReplayEventKind, ReplayScope, ReplaySseFrame, StreamTerminalMarker,
    replay_sse_event_name, replay_sse_frames,
};

#[test]
fn replay_sse_frames_serialize_full_events_with_canonical_names()
-> starweaver_stream::ReplayResult<()> {
    let scope = ReplayScope::run("run_sse");
    let events = vec![
        ReplayEvent::new(scope.clone(), 1, ReplayEventKind::Heartbeat),
        ReplayEvent::new(
            scope.clone(),
            2,
            ReplayEventKind::Raw(json!({ "delta": "ok" })),
        ),
        ReplayEvent::new(
            scope,
            3,
            ReplayEventKind::Terminal {
                marker: StreamTerminalMarker::RunCompleted,
            },
        ),
    ];

    let frames = replay_sse_frames(&events)?;
    assert_eq!(frames[0].id, "1");
    assert_eq!(frames[0].event, "heartbeat");
    assert!(frames[0].data.contains("\"sequence\":1"));
    assert_eq!(frames[1].event, "raw");
    assert_eq!(frames[2].event, "terminal");
    assert!(frames[2].to_frame().contains("event: terminal"));
    Ok(())
}

#[test]
fn ready_sse_frame_has_empty_object_payload() {
    let frame = ReplaySseFrame::ready();
    assert_eq!(frame.event, "ready");
    assert_eq!(frame.data, "{}");
    assert_eq!(frame.to_frame(), "event: ready\ndata: {}\n\n");
}

#[test]
fn replay_sse_event_names_cover_all_builtin_kinds() {
    assert_eq!(
        replay_sse_event_name(&ReplayEventKind::Heartbeat),
        "heartbeat"
    );
    assert_eq!(
        replay_sse_event_name(&ReplayEventKind::Terminal {
            marker: StreamTerminalMarker::RunCancelled {
                reason: "test".to_string(),
            },
        }),
        "terminal"
    );
}
