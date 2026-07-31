#![allow(clippy::expect_used)]

use starweaver_model::ToolReturnPart;

use super::{AgentStreamEvent, AgentStreamRecord, project_stream_records_for_durable_evidence};

const SCREENSHOT_DATA_URL: &str =
    "data:image/png;base64,Q0xJX1JBV19TVFJFQU1fU0NSRUVOU0hPVF9TRU5USU5FTA==";

fn screenshot_record() -> AgentStreamRecord {
    AgentStreamRecord::new(
        7,
        AgentStreamEvent::ToolReturn {
            step: 1,
            tool_return: ToolReturnPart::new(
                "call-screenshot",
                "computer_observe",
                serde_json::json!({"observation_id": "observation-1"}),
            )
            .with_private_metadata(serde_json::Map::from_iter([
                (
                    "starweaver_geometry_bound_immutable_media".to_string(),
                    serde_json::json!(true),
                ),
                (
                    "starweaver_tool_return_content_parts".to_string(),
                    serde_json::json!([{
                        "kind": "data_url",
                        "data_url": SCREENSHOT_DATA_URL,
                        "media_type": "image/png"
                    }]),
                ),
                (
                    "retained_audit_marker".to_string(),
                    serde_json::json!("keep"),
                ),
            ])),
        },
    )
}

#[test]
fn durable_stream_projection_strips_screenshot_clone_without_mutating_live_record() {
    let live_records = vec![screenshot_record()];

    let durable_records = project_stream_records_for_durable_evidence(&live_records);

    let live_json = serde_json::to_string(&live_records).expect("serialize live records");
    let durable_json = serde_json::to_string(&durable_records).expect("serialize durable records");
    assert!(live_json.contains(SCREENSHOT_DATA_URL));
    assert!(!durable_json.contains(SCREENSHOT_DATA_URL));
    assert!(durable_json.contains("retained_audit_marker"));
}
