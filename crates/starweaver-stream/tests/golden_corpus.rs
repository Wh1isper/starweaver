#![allow(missing_docs, clippy::expect_used)]

use serde_json::{Value, json};
use starweaver_core::{RunId, SessionId};
use starweaver_stream::{
    AgentStreamRecord, DefaultDisplayMessageProjector, DisplayProjectionContext, JsonlEnvelope,
    ReplayEvent, ReplayEventKind, ReplayScope, StreamTerminalMarker,
};

const GOLDEN: &str = include_str!("../../../spec/fixtures/stream/raw-display-replay-v1.json");

#[test]
#[allow(clippy::too_many_lines)]
fn shared_golden_corpus_freezes_raw_display_and_replay_contracts() {
    let fixture: Value = serde_json::from_str(GOLDEN).expect("parse shared stream fixture");
    assert_eq!(
        fixture["schema"],
        "starweaver.fixture.raw-display-replay.v1"
    );
    let raw = fixture["raw_records"]
        .as_array()
        .expect("raw_records array")
        .clone();
    let records: Vec<AgentStreamRecord> =
        serde_json::from_value(Value::Array(raw.clone())).expect("decode canonical raw records");

    let context = DisplayProjectionContext::new(
        SessionId::from_string(fixture["session_id"].as_str().expect("session id")),
        RunId::from_string(fixture["run_id"].as_str().expect("run id")),
    );
    let display = DefaultDisplayMessageProjector.project_records(&context, &records);
    assert_eq!(
        display.iter().map(|item| item.sequence).collect::<Vec<_>>(),
        json_to_usizes(&fixture["display"]["sequences"])
    );
    assert_eq!(
        display
            .iter()
            .map(|item| serde_json::to_value(item.kind).expect("serialize display kind"))
            .collect::<Vec<_>>(),
        fixture["display"]["types"]
            .as_array()
            .expect("display types")
            .clone()
    );
    assert_eq!(
        display
            .iter()
            .map(|item| item.payload.clone())
            .collect::<Vec<_>>(),
        fixture["display"]["payloads"]
            .as_array()
            .expect("display payloads")
            .clone()
    );
    let terminal = display.last().expect("terminal display message");
    assert!(terminal.is_terminal());
    assert_eq!(
        serde_json::to_value(terminal.kind).expect("serialize terminal kind"),
        fixture["display"]["terminal_type"]
    );

    let source = &fixture["display"]["source"];
    let display_index = usize::try_from(source["display_index"].as_u64().expect("display index"))
        .expect("display index fits usize");
    let sourced = &display[display_index];
    assert_eq!(
        sourced
            .agent_id
            .as_ref()
            .map(starweaver_core::AgentId::as_str),
        source["agent_id"].as_str()
    );
    assert_eq!(sourced.agent_name.as_deref(), source["agent_name"].as_str());
    assert_eq!(
        sourced.run_id.as_str(),
        source["run_id"].as_str().expect("source run id")
    );
    assert_eq!(
        sourced.metadata.get("source_sequence"),
        Some(&source["source_sequence"])
    );

    let scope = ReplayScope::run(fixture["run_id"].as_str().expect("run id"));
    let mut replay = raw
        .into_iter()
        .enumerate()
        .map(|(sequence, value)| {
            ReplayEvent::new(scope.clone(), sequence, ReplayEventKind::Raw(value))
        })
        .collect::<Vec<_>>();
    replay.push(ReplayEvent::new(
        scope,
        replay.len(),
        ReplayEventKind::Terminal {
            marker: StreamTerminalMarker::RunCompleted,
        },
    ));
    let envelopes = replay
        .iter()
        .map(JsonlEnvelope::from_event)
        .collect::<Vec<_>>();
    assert_eq!(
        envelopes
            .iter()
            .map(|item| item.sequence)
            .collect::<Vec<_>>(),
        json_to_usizes(&fixture["replay"]["sequences"])
    );
    assert_eq!(
        envelopes
            .iter()
            .map(|item| Value::String(item.kind.clone()))
            .collect::<Vec<_>>(),
        fixture["replay"]["kinds"]
            .as_array()
            .expect("replay kinds")
            .clone()
    );
    assert_eq!(
        envelopes.last().expect("terminal replay envelope").data,
        fixture["replay"]["terminal"]
    );
    assert_eq!(
        envelopes[0]
            .cursor
            .as_ref()
            .expect("replay cursor")
            .scope
            .as_str(),
        fixture["replay"]["scope"].as_str().expect("replay scope")
    );
    assert_eq!(
        envelopes[1].data["source"],
        json!({
            "kind": "subagent",
            "agent_id": "agent-researcher",
            "agent_name": "researcher",
            "task_id": "task-golden",
            "run_id": "run-child-golden",
            "parent_run_id": "run-golden",
            "source_sequence": 7
        })
    );

    let cancelled_record: AgentStreamRecord =
        serde_json::from_value(fixture["cancelled"]["raw_record"].clone())
            .expect("decode cancelled raw record");
    let cancelled = DefaultDisplayMessageProjector.project_records(&context, &[cancelled_record]);
    assert_eq!(cancelled.len(), 1);
    assert_eq!(
        cancelled[0].sequence,
        fixture["cancelled"]["display"]["sequence"]
    );
    assert_eq!(
        serde_json::to_value(cancelled[0].kind).expect("serialize cancellation kind"),
        fixture["cancelled"]["display"]["type"]
    );
    assert_eq!(
        cancelled[0].payload,
        fixture["cancelled"]["display"]["payload"]
    );
    assert!(cancelled[0].is_terminal());
    assert_eq!(
        serde_json::to_value(StreamTerminalMarker::RunCancelled {
            reason: "cancelled by fixture".to_string(),
        })
        .expect("serialize replay cancellation marker"),
        fixture["cancelled"]["replay_terminal"]
    );
}

fn json_to_usizes(value: &Value) -> Vec<usize> {
    value
        .as_array()
        .expect("sequence array")
        .iter()
        .map(|item| {
            usize::try_from(item.as_u64().expect("sequence integer")).expect("sequence fits usize")
        })
        .collect()
}
