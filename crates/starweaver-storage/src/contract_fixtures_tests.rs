use serde_json::Value;
use starweaver_context::{AgentCheckpoint, ResumableState};
use starweaver_core::VersionedRecord;
use starweaver_stream::AgentStreamRecord;

use crate::sqlite::{deserialize_json_record, serialize_json_record};

const CONTEXT_V0: &str = include_str!("../tests/fixtures/contracts/resumable-state-v0.json");
const CONTEXT_V1: &str = include_str!("../tests/fixtures/contracts/resumable-state-v1.json");
const CONTEXT_UNKNOWN: &str =
    include_str!("../tests/fixtures/contracts/resumable-state-unknown-version.json");
const CONTEXT_WRONG_SCHEMA: &str =
    include_str!("../tests/fixtures/contracts/resumable-state-wrong-schema.json");
const CHECKPOINT_V0: &str = include_str!("../tests/fixtures/contracts/checkpoint-v0.json");
const CHECKPOINT_V1: &str = include_str!("../tests/fixtures/contracts/checkpoint-v1.json");
const STREAM_V0: &str = include_str!("../tests/fixtures/contracts/stream-record-v0.json");
const STREAM_V1: &str = include_str!("../tests/fixtures/contracts/stream-record-v1.json");

#[test]
fn storage_codecs_read_v0_and_v1_and_write_current_envelopes() {
    assert_storage_fixture::<ResumableState>(CONTEXT_V0, CONTEXT_V1);
    assert_storage_fixture::<AgentCheckpoint>(CHECKPOINT_V0, CHECKPOINT_V1);
    assert_storage_fixture::<AgentStreamRecord>(STREAM_V0, STREAM_V1);
}

fn assert_storage_fixture<T>(legacy: &str, current: &str)
where
    T: serde::de::DeserializeOwned
        + serde::Serialize
        + VersionedRecord
        + std::fmt::Debug
        + PartialEq,
{
    let legacy = deserialize_json_record::<T>(legacy).expect("read legacy storage record");
    let current_value = deserialize_json_record::<T>(current).expect("read current storage record");
    assert_eq!(legacy, current_value);
    let encoded = serialize_json_record(&legacy).expect("write current storage record");
    assert_eq!(
        serde_json::from_str::<Value>(&encoded).expect("parse encoded storage record"),
        serde_json::from_str::<Value>(current).expect("parse current fixture")
    );
}

#[test]
fn storage_codecs_map_unknown_versions_and_wrong_schemas_to_safe_errors() {
    let unknown = deserialize_json_record::<ResumableState>(CONTEXT_UNKNOWN)
        .expect_err("unknown versions must fail");
    assert!(unknown.to_string().contains("unsupported"));
    let wrong = deserialize_json_record::<ResumableState>(CONTEXT_WRONG_SCHEMA)
        .expect_err("wrong schemas must fail");
    assert!(wrong.to_string().contains("expected durable schema"));
}
