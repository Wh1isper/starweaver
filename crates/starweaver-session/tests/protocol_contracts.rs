#![allow(missing_docs, clippy::expect_used)]

use serde::Deserialize;
use serde_json::Value;
use starweaver_core::{
    RunId, RunLifecycle, VersionedRecordError, from_versioned_json, to_versioned_value,
};
use starweaver_model::ContentPart;
use starweaver_session::{
    ApprovalRecord, DeferredToolRecord, DurableRunStatus, InputConversionError, InputPart,
    RunRecord, SessionRecord, StreamCursorRef,
};
use starweaver_stream::ReplayCursorFamily;

const INPUT: &str = include_str!("fixtures/contracts/canonical-input-v1.json");
const LEGACY_INPUT: &str = include_str!("fixtures/contracts/legacy-input-v0.json");
const LIFECYCLE: &str = include_str!("fixtures/contracts/lifecycle-v1.json");
const CURSOR_V0: &str = include_str!("fixtures/contracts/cursor-ref-v0.json");
const CURSOR_V1: &str = include_str!("fixtures/contracts/cursor-ref-v1.json");
const CURSOR_MIXED: &str = include_str!("fixtures/contracts/cursor-ref-mixed-invalid.json");
const SESSION_V0: &str = include_str!("fixtures/contracts/session-record-v0.json");
const SESSION_V1: &str = include_str!("fixtures/contracts/session-record-v1.json");
const RUN_V0: &str = include_str!("fixtures/contracts/run-record-v0.json");
const RUN_V1: &str = include_str!("fixtures/contracts/run-record-v1.json");
const APPROVAL_V0: &str = include_str!("fixtures/contracts/approval-record-v0.json");
const APPROVAL_V1: &str = include_str!("fixtures/contracts/approval-record-v1.json");
const DEFERRED_V0: &str = include_str!("fixtures/contracts/deferred-record-v0.json");
const DEFERRED_V1: &str = include_str!("fixtures/contracts/deferred-record-v1.json");
const RUN_UNKNOWN: &str = include_str!("fixtures/contracts/run-record-unknown-version.json");
const RUN_WRONG_SCHEMA: &str = include_str!("fixtures/contracts/run-record-wrong-schema.json");

#[derive(Deserialize)]
struct LifecycleFixture {
    runtime: Vec<String>,
    durable: Vec<String>,
}

#[test]
fn canonical_input_fixture_covers_every_content_variant_losslessly() {
    let inputs = serde_json::from_str::<Vec<InputPart>>(INPUT).expect("read canonical input");
    let content = inputs
        .clone()
        .into_iter()
        .map(ContentPart::try_from)
        .collect::<Result<Vec<_>, _>>()
        .expect("convert canonical input");
    assert_eq!(content.len(), 7);
    assert!(matches!(content[0], ContentPart::CachePoint { .. }));
    assert!(matches!(content[1], ContentPart::Text { .. }));
    assert!(matches!(content[2], ContentPart::ImageUrl { .. }));
    assert!(matches!(content[3], ContentPart::FileUrl { .. }));
    assert!(matches!(content[4], ContentPart::Binary { .. }));
    assert!(matches!(content[5], ContentPart::ResourceRef { .. }));
    assert!(matches!(content[6], ContentPart::DataUrl { .. }));
    assert_eq!(
        content.into_iter().map(InputPart::from).collect::<Vec<_>>(),
        inputs
    );
}

#[test]
fn legacy_input_fixture_has_explicit_conversion_and_product_edge_failures() {
    let inputs = serde_json::from_str::<Vec<InputPart>>(LEGACY_INPUT).expect("read legacy input");
    for input in inputs.iter().take(3).cloned() {
        ContentPart::try_from(input).expect("legacy content evidence converts explicitly");
    }
    assert!(matches!(
        ContentPart::try_from(inputs[3].clone()),
        Err(InputConversionError::ProductMode(mode)) if mode == "content_part"
    ));
    assert!(matches!(
        ContentPart::try_from(inputs[4].clone()),
        Err(InputConversionError::ProductMode(mode)) if mode == "plan"
    ));
    assert!(matches!(
        ContentPart::try_from(inputs[5].clone()),
        Err(InputConversionError::ProductCommand(command)) if command == "review"
    ));
}

#[test]
fn lifecycle_fixture_covers_runtime_and_durable_wire_vocabularies() {
    let fixture = serde_json::from_str::<LifecycleFixture>(LIFECYCLE).expect("read lifecycle");
    let runtime = fixture
        .runtime
        .iter()
        .map(|value| serde_json::from_value::<RunLifecycle>(Value::String(value.clone())))
        .collect::<Result<Vec<_>, _>>()
        .expect("decode runtime lifecycle");
    assert_eq!(runtime.len(), 6);
    assert!(runtime.iter().all(|state| state.as_str() != "queued"));

    let durable = fixture
        .durable
        .iter()
        .map(|value| serde_json::from_value::<DurableRunStatus>(Value::String(value.clone())))
        .collect::<Result<Vec<_>, _>>()
        .expect("decode durable lifecycle");
    assert_eq!(durable.len(), 7);
    assert_eq!(durable[0], DurableRunStatus::Queued);
    assert!(durable[4..].iter().all(|status| status.is_terminal()));
}

#[test]
fn flat_v0_and_composed_v1_cursor_refs_decode_to_the_same_position() {
    let legacy = serde_json::from_str::<StreamCursorRef>(CURSOR_V0).expect("read flat cursor");
    let current = serde_json::from_str::<StreamCursorRef>(CURSOR_V1).expect("read composed cursor");
    assert_eq!(legacy, current);
    assert_eq!(current.family(), ReplayCursorFamily::Display);
    assert_eq!(current.scope().as_str(), "run:run-fixture");
    assert_eq!(current.sequence(), 7);
    assert_eq!(
        serde_json::to_value(legacy).expect("write cursor"),
        serde_json::from_str::<Value>(CURSOR_V1).expect("parse current cursor")
    );
}

#[test]
fn cursor_updates_reject_mixed_shapes_wrong_runs_and_sequence_regression() {
    assert!(serde_json::from_str::<StreamCursorRef>(CURSOR_MIXED).is_err());
    let current = serde_json::from_str::<StreamCursorRef>(CURSOR_V1).expect("read cursor");
    assert!(
        current
            .validate_for_run(&RunId::from_string("other-run"))
            .is_err()
    );
    let stale = StreamCursorRef::new(starweaver_stream::ReplayCursor::display(
        current.scope().clone(),
        current.sequence() - 1,
    ));
    assert!(stale.validate_progression(&current).is_err());
}

#[test]
fn durable_records_read_v0_and_v1_and_write_current_envelopes() {
    assert_v0_v1::<SessionRecord>(SESSION_V0, SESSION_V1);
    assert_v0_v1::<RunRecord>(RUN_V0, RUN_V1);
    assert_v0_v1::<ApprovalRecord>(APPROVAL_V0, APPROVAL_V1);
    assert_v0_v1::<DeferredToolRecord>(DEFERRED_V0, DEFERRED_V1);
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
fn durable_records_reject_unknown_versions_and_wrong_schemas() {
    assert!(matches!(
        from_versioned_json::<RunRecord>(RUN_UNKNOWN),
        Err(VersionedRecordError::UnsupportedVersion { actual: 2, .. })
    ));
    assert!(matches!(
        from_versioned_json::<RunRecord>(RUN_WRONG_SCHEMA),
        Err(VersionedRecordError::WrongSchema { .. })
    ));
}
