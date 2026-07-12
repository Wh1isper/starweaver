#![allow(missing_docs, clippy::expect_used)]

use serde_json::Value;
use starweaver_core::{
    AgentExecutionNode, RunLifecycle, VersionedRecordError, from_versioned_json, to_versioned_value,
};
use starweaver_stream::{
    AgentSidebandEventCategory, AgentStreamEvent, AgentStreamRecord, AgentStreamSourceKind,
    ReplayCursor, ReplayCursorFamily, ReplayEvent, ReplayScope, ReplaySnapshot,
};

const EVENT_V0: &str = include_str!("fixtures/contracts/replay-event-v0.json");
const EVENT_V1: &str = include_str!("fixtures/contracts/replay-event-v1.json");
const SNAPSHOT_V0: &str = include_str!("fixtures/contracts/replay-snapshot-v0.json");
const SNAPSHOT_V1: &str = include_str!("fixtures/contracts/replay-snapshot-v1.json");
const EVENT_UNKNOWN: &str = include_str!("fixtures/contracts/replay-event-unknown-version.json");
const EVENT_WRONG_SCHEMA: &str = include_str!("fixtures/contracts/replay-event-wrong-schema.json");
const STREAM_NODE_SOURCE_V0: &str =
    include_str!("fixtures/contracts/stream-record-node-source-v0.json");
const STREAM_NODE_SOURCE_V1: &str =
    include_str!("fixtures/contracts/stream-record-node-source-v1.json");
const STREAM_CUSTOM_V0: &str = include_str!("fixtures/contracts/stream-record-custom-v0.json");
const STREAM_CUSTOM_V1: &str = include_str!("fixtures/contracts/stream-record-custom-v1.json");
const STREAM_CUSTOM_DEFAULT_METADATA_V0: &str =
    include_str!("fixtures/contracts/stream-record-custom-default-metadata-v0.json");
const STREAM_CUSTOM_DEFAULT_METADATA_V1: &str =
    include_str!("fixtures/contracts/stream-record-custom-default-metadata-v1.json");
const STREAM_UNKNOWN: &str = include_str!("fixtures/contracts/stream-record-unknown-version.json");
const STREAM_WRONG_SCHEMA: &str =
    include_str!("fixtures/contracts/stream-record-wrong-schema.json");

#[test]
fn replay_records_read_v0_and_v1_and_write_current_envelopes() {
    assert_v0_v1::<ReplayEvent>(EVENT_V0, EVENT_V1);
    assert_v0_v1::<ReplaySnapshot>(SNAPSHOT_V0, SNAPSHOT_V1);
}

fn assert_v0_v1<T>(legacy: &str, current: &str)
where
    T: serde::de::DeserializeOwned
        + serde::Serialize
        + starweaver_core::VersionedRecord
        + std::fmt::Debug
        + PartialEq,
{
    let legacy = from_versioned_json::<T>(legacy).expect("read v0 record");
    let current_value = from_versioned_json::<T>(current).expect("read v1 record");
    assert_eq!(legacy, current_value);
    assert_eq!(
        to_versioned_value(&legacy).expect("write current record"),
        serde_json::from_str::<Value>(current).expect("parse current fixture")
    );
}

#[test]
fn raw_stream_records_freeze_owner_wire_shapes() {
    assert_v0_v1::<AgentStreamRecord>(STREAM_NODE_SOURCE_V0, STREAM_NODE_SOURCE_V1);
    assert_v0_v1::<AgentStreamRecord>(STREAM_CUSTOM_V0, STREAM_CUSTOM_V1);
    assert_v0_v1::<AgentStreamRecord>(
        STREAM_CUSTOM_DEFAULT_METADATA_V0,
        STREAM_CUSTOM_DEFAULT_METADATA_V1,
    );

    let node_record =
        from_versioned_json::<AgentStreamRecord>(STREAM_NODE_SOURCE_V1).expect("read node record");
    let source = node_record.source.expect("fixture has source attribution");
    assert_eq!(source.kind, AgentStreamSourceKind::Subagent);
    assert_eq!(source.source_sequence, 2);
    assert!(matches!(
        node_record.event,
        AgentStreamEvent::NodeStart {
            node: AgentExecutionNode::PrepareModelRequest,
            step: 3,
            status: RunLifecycle::Running,
        }
    ));

    let custom =
        from_versioned_json::<AgentStreamRecord>(STREAM_CUSTOM_V1).expect("read custom record");
    assert_eq!(
        custom
            .event
            .sideband_event()
            .expect("known custom event has typed sideband view")
            .category,
        AgentSidebandEventCategory::Tool
    );
    let default_metadata =
        from_versioned_json::<AgentStreamRecord>(STREAM_CUSTOM_DEFAULT_METADATA_V1)
            .expect("read default-metadata custom record");
    let AgentStreamEvent::Custom { event } = default_metadata.event else {
        panic!("fixture must contain a custom event");
    };
    assert!(event.metadata.is_empty());
}

#[test]
fn raw_stream_records_reject_unknown_versions_and_wrong_schemas() {
    assert!(matches!(
        from_versioned_json::<AgentStreamRecord>(STREAM_UNKNOWN),
        Err(VersionedRecordError::UnsupportedVersion { actual: 2, .. })
    ));
    assert!(matches!(
        from_versioned_json::<AgentStreamRecord>(STREAM_WRONG_SCHEMA),
        Err(VersionedRecordError::WrongSchema { .. })
    ));
}

#[test]
fn replay_records_reject_unknown_versions_and_wrong_schemas() {
    assert!(matches!(
        from_versioned_json::<ReplayEvent>(EVENT_UNKNOWN),
        Err(VersionedRecordError::UnsupportedVersion { actual: 2, .. })
    ));
    assert!(matches!(
        from_versioned_json::<ReplayEvent>(EVENT_WRONG_SCHEMA),
        Err(VersionedRecordError::WrongSchema { .. })
    ));
}

#[test]
fn cursor_family_is_part_of_replay_compatibility() {
    let scope = ReplayScope::run("run-fixture");
    let display = ReplayCursor::display(scope.clone(), 3);
    assert!(
        display
            .validate(ReplayCursorFamily::Display, &scope)
            .is_ok()
    );
    assert!(
        display
            .validate(ReplayCursorFamily::ReplayEvent, &scope)
            .is_err()
    );
    assert!(
        display
            .validate(ReplayCursorFamily::Display, &ReplayScope::run("other-run"))
            .is_err()
    );
}
