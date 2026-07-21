#![allow(missing_docs, clippy::expect_used)]

use serde_json::Value;
use starweaver_core::{RunId, SessionId};
use starweaver_rpc_core::replay_result;
use starweaver_runtime::AgentStreamRecord;
use starweaver_stream::{
    DefaultDisplayMessageProjector, DisplayProjectionContext, ReplayEvent, ReplayEventKind,
    ReplayScope, StreamTerminalMarker,
};

const GOLDEN: &str = include_str!("../../../spec/fixtures/stream/raw-display-replay-v1.json");

#[test]
#[allow(clippy::too_many_lines)]
fn rpc_replay_result_consumes_shared_stream_golden_corpus() {
    let fixture: Value = serde_json::from_str(GOLDEN).expect("parse shared stream fixture");
    let records: Vec<AgentStreamRecord> =
        serde_json::from_value(fixture["raw_records"].clone()).expect("decode raw records");
    let context = DisplayProjectionContext::new(
        SessionId::from_string(fixture["session_id"].as_str().expect("session id")),
        RunId::from_string(fixture["run_id"].as_str().expect("run id")),
    );
    let messages = DefaultDisplayMessageProjector.project_records(&context, &records);
    let scope = ReplayScope::run(fixture["run_id"].as_str().expect("run id"));
    let mut events = messages
        .into_iter()
        .enumerate()
        .map(|(sequence, message)| ReplayEvent::display_at(scope.clone(), sequence, message))
        .collect::<Vec<_>>();
    events.push(ReplayEvent::new(
        scope.clone(),
        events.len(),
        ReplayEventKind::Terminal {
            marker: StreamTerminalMarker::RunCompleted,
        },
    ));

    let terminal_index = events.len() - 1;
    let result = replay_result(
        fixture["session_id"].as_str().expect("session id"),
        fixture["run_id"].as_str(),
        &scope,
        &events,
        None,
        events.len(),
    );
    assert_eq!(result["scope"], fixture["replay"]["scope"]);
    assert_eq!(result["nextSequence"], events.len());
    assert_eq!(
        result["events"]
            .as_array()
            .expect("RPC replay events")
            .iter()
            .map(|event| event["sequence"].clone())
            .collect::<Vec<_>>(),
        fixture["replay"]["sequences"]
            .as_array()
            .expect("golden replay sequences")
            .clone()
    );
    assert_eq!(
        result["messages"]
            .as_array()
            .expect("RPC display messages")
            .iter()
            .map(|message| message["type"].clone())
            .collect::<Vec<_>>(),
        fixture["display"]["types"]
            .as_array()
            .expect("golden display types")
            .clone()
    );
    assert_eq!(
        result["events"][terminal_index]["event"]["marker"],
        fixture["replay"]["terminal"]
    );
    assert_eq!(
        result["events"][terminal_index]["event"]["kind"],
        "terminal"
    );

    let source = &fixture["display"]["source"];
    let display_index = usize::try_from(source["display_index"].as_u64().expect("display index"))
        .expect("display index fits usize");
    let sourced = &result["messages"][display_index];
    assert_eq!(sourced["agent_id"], source["agent_id"]);
    assert_eq!(sourced["run_id"], source["run_id"]);
    assert_eq!(
        sourced["metadata"]["source_sequence"],
        source["source_sequence"]
    );
    assert_eq!(result["latestCursor"]["sequence"], terminal_index);
    assert_eq!(result["latestCursor"]["scope"], fixture["replay"]["scope"]);

    let cancelled_record: AgentStreamRecord =
        serde_json::from_value(fixture["cancelled"]["raw_record"].clone())
            .expect("decode cancelled raw record");
    let cancelled = DefaultDisplayMessageProjector.project_records(&context, &[cancelled_record]);
    let cancelled_events = vec![
        ReplayEvent::display_at(scope.clone(), 0, cancelled[0].clone()),
        ReplayEvent::new(
            scope.clone(),
            1,
            ReplayEventKind::Terminal {
                marker: StreamTerminalMarker::RunCancelled {
                    reason: "cancelled by fixture".to_string(),
                },
            },
        ),
    ];
    let cancelled_result = replay_result(
        fixture["session_id"].as_str().expect("session id"),
        fixture["run_id"].as_str(),
        &scope,
        &cancelled_events,
        None,
        2,
    );
    assert_eq!(
        cancelled_result["messages"][0]["type"],
        fixture["cancelled"]["display"]["type"]
    );
    assert_eq!(
        cancelled_result["messages"][0]["payload"],
        fixture["cancelled"]["display"]["payload"]
    );
    assert_eq!(
        cancelled_result["events"][1]["event"]["marker"],
        fixture["cancelled"]["replay_terminal"]
    );
}
