#![allow(missing_docs, clippy::expect_used)]

use serde_json::Value;
use starweaver_context::AgentCheckpoint;
use starweaver_core::{VersionedRecordError, from_versioned_json, to_versioned_value};

const CHECKPOINT_V0: &str = include_str!("fixtures/contracts/checkpoint-v0.json");
const CHECKPOINT_V1: &str = include_str!("fixtures/contracts/checkpoint-v1.json");
const CHECKPOINT_UNKNOWN: &str = include_str!("fixtures/contracts/checkpoint-unknown-version.json");
const CHECKPOINT_WRONG_SCHEMA: &str =
    include_str!("fixtures/contracts/checkpoint-wrong-schema.json");

#[test]
fn checkpoint_owner_reads_v0_and_v1_and_writes_current_envelope() {
    let legacy = from_versioned_json::<AgentCheckpoint>(CHECKPOINT_V0).expect("read v0 checkpoint");
    let current =
        from_versioned_json::<AgentCheckpoint>(CHECKPOINT_V1).expect("read v1 checkpoint");

    assert_eq!(legacy, current);
    assert_eq!(
        to_versioned_value(&legacy).expect("write current checkpoint"),
        serde_json::from_str::<Value>(CHECKPOINT_V1).expect("parse current fixture")
    );
}

#[test]
fn checkpoint_owner_rejects_unknown_versions_and_wrong_schemas() {
    assert!(matches!(
        from_versioned_json::<AgentCheckpoint>(CHECKPOINT_UNKNOWN),
        Err(VersionedRecordError::UnsupportedVersion { actual: 2, .. })
    ));
    assert!(matches!(
        from_versioned_json::<AgentCheckpoint>(CHECKPOINT_WRONG_SCHEMA),
        Err(VersionedRecordError::WrongSchema { .. })
    ));
}
