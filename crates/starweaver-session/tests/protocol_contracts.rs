#![allow(missing_docs, clippy::expect_used)]

use serde::Deserialize;
use serde_json::Value;
use starweaver_core::{
    RunId, RunLifecycle, VersionedRecordError, from_versioned_json, to_versioned_value,
};
use starweaver_model::ContentPart;
use starweaver_session::{
    ApprovalRecord, DeferredToolRecord, DurableRunStatus, InputConversionError, InputPart,
    RunRecord, RuntimeConfigSnapshotRef, SessionRecord, StreamCursorRef, WorkspaceProvenanceRef,
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
const SESSION_V1_WORKSPACE: &str =
    include_str!("fixtures/contracts/session-record-v1-workspace.json");
const SESSION_V2: &str = include_str!("fixtures/contracts/session-record-v2.json");
const SESSION_V2_WORKSPACE: &str =
    include_str!("fixtures/contracts/session-record-v2-workspace.json");
const RUN_V0: &str = include_str!("fixtures/contracts/run-record-v0.json");
const RUN_V1: &str = include_str!("fixtures/contracts/run-record-v1.json");
const RUN_V2: &str = include_str!("fixtures/contracts/run-record-v2.json");
const RUN_V2_CONFIG: &str = include_str!("fixtures/contracts/run-record-v2-config.json");
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
fn durable_records_read_previous_versions_and_write_current_envelopes() {
    assert_previous_current::<SessionRecord>(SESSION_V0, SESSION_V2);
    assert_previous_current::<SessionRecord>(SESSION_V1, SESSION_V2);
    assert_previous_current::<RunRecord>(RUN_V0, RUN_V2);
    assert_previous_current::<RunRecord>(RUN_V1, RUN_V2);
    assert_previous_current::<ApprovalRecord>(APPROVAL_V0, APPROVAL_V1);
    assert_previous_current::<DeferredToolRecord>(DEFERRED_V0, DEFERRED_V1);
}

fn assert_previous_current<T>(legacy: &str, current: &str)
where
    T: serde::de::DeserializeOwned
        + serde::Serialize
        + starweaver_core::VersionedRecord
        + std::fmt::Debug
        + PartialEq,
{
    let legacy = from_versioned_json::<T>(legacy).expect("read previous record");
    let current_value = from_versioned_json::<T>(current).expect("read current record");
    assert_eq!(legacy, current_value);
    assert_eq!(
        to_versioned_value(&legacy).expect("write current record"),
        serde_json::from_str::<Value>(current).expect("parse current fixture")
    );
}

#[test]
fn canonical_workspace_provenance_is_stable_and_execution_domain_bound() {
    let first = WorkspaceProvenanceRef::for_execution_domain_root(
        "standalone-local",
        "/workspace/project",
        Some("Project".to_string()),
    );
    let relabeled = WorkspaceProvenanceRef::for_execution_domain_root(
        "standalone-local",
        "/workspace/project",
        Some("Renamed".to_string()),
    );
    let other_domain = WorkspaceProvenanceRef::for_execution_domain_root(
        "remote-domain",
        "/workspace/project",
        None,
    );

    assert_eq!(first.workspace_id, relabeled.workspace_id);
    assert_eq!(first.provenance_digest, relabeled.provenance_digest);
    assert_ne!(first.workspace_id, other_domain.workspace_id);
    assert_ne!(first.provenance_digest, other_domain.provenance_digest);
    assert!(first.workspace_id.starts_with("workspace_"));
    assert!(first.provenance_digest.starts_with("sha256:"));
    assert!(!first.is_legacy_unbound());
    assert_eq!(first.validate(), Ok(()));
}

#[test]
fn workspace_and_config_v2_provenance_are_typed_and_authority_neutral() {
    let legacy = from_versioned_json::<SessionRecord>(SESSION_V1_WORKSPACE)
        .expect("migrate legacy workspace");
    let legacy_workspace = legacy.workspace.expect("legacy workspace provenance");
    assert!(legacy_workspace.is_legacy_unbound());
    assert_eq!(legacy_workspace.display_value(), "/legacy/project");
    assert!(legacy_workspace.provenance_digest.starts_with("sha256:"));

    let current = from_versioned_json::<SessionRecord>(SESSION_V2_WORKSPACE)
        .expect("read current workspace provenance");
    let current_workspace = current.workspace.expect("current workspace provenance");
    assert_eq!(current_workspace.workspace_id, "ws_01JABCDEF");
    assert!(!current_workspace.is_legacy_unbound());
    assert_eq!(current_workspace.display_value(), "project");

    let run =
        from_versioned_json::<RunRecord>(RUN_V2_CONFIG).expect("read config snapshot provenance");
    assert_eq!(
        run.config_snapshot,
        Some(RuntimeConfigSnapshotRef::new(
            7,
            "cfg_01JABCDEF",
            "sha256:materialization-fixture"
        ))
    );
}

#[test]
fn durable_records_reject_unknown_versions_and_wrong_schemas() {
    assert!(matches!(
        from_versioned_json::<RunRecord>(RUN_UNKNOWN),
        Err(VersionedRecordError::UnsupportedVersion { actual: 3, .. })
    ));
    assert!(matches!(
        from_versioned_json::<RunRecord>(RUN_WRONG_SCHEMA),
        Err(VersionedRecordError::WrongSchema { .. })
    ));
}
