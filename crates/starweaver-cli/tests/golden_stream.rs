#![allow(missing_docs, clippy::expect_used)]

use serde_json::Value;
use starweaver_core::{RunId, SessionId};
use starweaver_runtime::AgentStreamRecord;
use starweaver_stream::{DefaultDisplayMessageProjector, DisplayProjectionContext};

const GOLDEN: &str = include_str!("../../../spec/fixtures/stream/raw-display-replay-v1.json");

#[test]
fn cli_jsonl_contract_consumes_shared_stream_golden_corpus() {
    let fixture: Value = serde_json::from_str(GOLDEN).expect("parse shared stream fixture");
    let records: Vec<AgentStreamRecord> =
        serde_json::from_value(fixture["raw_records"].clone()).expect("decode raw records");
    let context = DisplayProjectionContext::new(
        SessionId::from_string(fixture["session_id"].as_str().expect("session id")),
        RunId::from_string(fixture["run_id"].as_str().expect("run id")),
    );
    let messages = DefaultDisplayMessageProjector.project_records(&context, &records);
    let jsonl = messages
        .iter()
        .map(|message| message.to_jsonl_line().expect("encode CLI JSONL"))
        .collect::<String>();
    let rendered = jsonl
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("decode CLI JSONL"))
        .collect::<Vec<_>>();

    assert_eq!(
        rendered
            .iter()
            .map(|message| message["sequence"].clone())
            .collect::<Vec<_>>(),
        fixture["display"]["sequences"]
            .as_array()
            .expect("display sequences")
            .clone()
    );
    assert_eq!(
        rendered
            .iter()
            .map(|message| message["type"].clone())
            .collect::<Vec<_>>(),
        fixture["display"]["types"]
            .as_array()
            .expect("display types")
            .clone()
    );
    assert_eq!(
        rendered
            .iter()
            .map(|message| message["payload"].clone())
            .collect::<Vec<_>>(),
        fixture["display"]["payloads"]
            .as_array()
            .expect("display payloads")
            .clone()
    );
    let terminal = rendered.last().expect("terminal message");
    assert_eq!(terminal["type"], fixture["display"]["terminal_type"]);

    let source = &fixture["display"]["source"];
    let display_index = usize::try_from(source["display_index"].as_u64().expect("display index"))
        .expect("display index fits usize");
    let sourced = &rendered[display_index];
    assert_eq!(sourced["agent_id"], source["agent_id"]);
    assert_eq!(sourced["agent_name"], source["agent_name"]);
    assert_eq!(sourced["run_id"], source["run_id"]);
    assert_eq!(
        sourced["metadata"]["source_sequence"],
        source["source_sequence"]
    );

    let cancelled_record: AgentStreamRecord =
        serde_json::from_value(fixture["cancelled"]["raw_record"].clone())
            .expect("decode cancelled raw record");
    let cancelled = DefaultDisplayMessageProjector.project_records(&context, &[cancelled_record]);
    let cancelled_json: Value = serde_json::from_str(
        cancelled[0]
            .to_jsonl_line()
            .expect("encode cancelled CLI JSONL")
            .trim(),
    )
    .expect("decode cancelled CLI JSONL");
    assert_eq!(
        cancelled_json["sequence"],
        fixture["cancelled"]["display"]["sequence"]
    );
    assert_eq!(
        cancelled_json["type"],
        fixture["cancelled"]["display"]["type"]
    );
    assert_eq!(
        cancelled_json["payload"],
        fixture["cancelled"]["display"]["payload"]
    );
}
