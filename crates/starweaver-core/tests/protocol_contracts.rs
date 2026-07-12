#![allow(missing_docs, clippy::expect_used)]

use serde::{Deserialize, Serialize};
use starweaver_core::{
    VersionedRecord, VersionedRecordError, from_versioned_json, to_versioned_value,
};

const V0: &str = include_str!("fixtures/contracts/record-v0.json");
const V1: &str = include_str!("fixtures/contracts/record-v1.json");
const UNKNOWN_VERSION: &str = include_str!("fixtures/contracts/record-unknown-version.json");
const WRONG_SCHEMA: &str = include_str!("fixtures/contracts/record-wrong-schema.json");
const VERSION_ZERO: &str = include_str!("fixtures/contracts/record-version-zero.json");
const MISSING_VERSION: &str = include_str!("fixtures/contracts/record-missing-version.json");
const MISSING_PAYLOAD: &str = include_str!("fixtures/contracts/record-missing-payload.json");
const MIGRATING_V1: &str = include_str!("fixtures/contracts/migrating-record-v1.json");
const MIGRATING_V2: &str = include_str!("fixtures/contracts/migrating-record-v2.json");

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
struct FixtureRecord {
    value: String,
}

impl VersionedRecord for FixtureRecord {
    const SCHEMA: &'static str = "starweaver.fixture.record";
    const ALLOW_BARE_V0: bool = true;
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
struct MigratingRecord {
    value: String,
    generation: u32,
}

#[derive(Deserialize)]
struct MigratingRecordV1 {
    legacy_value: String,
}

impl VersionedRecord for MigratingRecord {
    const SCHEMA: &'static str = "starweaver.fixture.migrating_record";
    const VERSION: u32 = 2;

    fn decode_version(
        version: u32,
        payload: serde_json::Value,
    ) -> Result<Self, VersionedRecordError> {
        match version {
            1 => {
                let legacy = serde_json::from_value::<MigratingRecordV1>(payload)
                    .map_err(VersionedRecordError::Json)?;
                Ok(Self {
                    value: legacy.legacy_value,
                    generation: 2,
                })
            }
            2 => serde_json::from_value(payload).map_err(VersionedRecordError::Json),
            actual => Err(VersionedRecordError::UnsupportedVersion {
                schema: Self::SCHEMA,
                supported: Self::VERSION,
                actual,
            }),
        }
    }
}

#[test]
fn reads_v0_and_v1_and_always_writes_current_envelope() {
    let legacy = from_versioned_json::<FixtureRecord>(V0).expect("read v0 fixture");
    let current = from_versioned_json::<FixtureRecord>(V1).expect("read v1 fixture");
    assert_eq!(legacy, current);
    assert_eq!(
        to_versioned_value(&legacy).expect("write current envelope"),
        serde_json::from_str::<serde_json::Value>(V1).expect("parse v1 fixture")
    );
}

#[test]
fn rejects_unknown_versions_and_wrong_schemas() {
    assert!(matches!(
        from_versioned_json::<FixtureRecord>(UNKNOWN_VERSION),
        Err(VersionedRecordError::UnsupportedVersion { actual: 2, .. })
    ));
    assert!(matches!(
        from_versioned_json::<FixtureRecord>(WRONG_SCHEMA),
        Err(VersionedRecordError::WrongSchema { .. })
    ));
    assert!(matches!(
        from_versioned_json::<FixtureRecord>(VERSION_ZERO),
        Err(VersionedRecordError::UnsupportedVersion { actual: 0, .. })
    ));
    assert!(matches!(
        from_versioned_json::<FixtureRecord>(MISSING_VERSION),
        Err(VersionedRecordError::Json(_))
    ));
    assert!(matches!(
        from_versioned_json::<FixtureRecord>(MISSING_PAYLOAD),
        Err(VersionedRecordError::Json(_))
    ));
}

#[test]
fn dispatches_older_enveloped_versions_through_a_dedicated_legacy_dto() {
    let migrated = from_versioned_json::<MigratingRecord>(MIGRATING_V1)
        .expect("migrate previous enveloped version");
    let current =
        from_versioned_json::<MigratingRecord>(MIGRATING_V2).expect("read current version");
    assert_eq!(migrated, current);
    assert_eq!(
        to_versioned_value(&migrated).expect("write migrated record"),
        serde_json::from_str::<serde_json::Value>(MIGRATING_V2)
            .expect("parse current migrating fixture")
    );
}

#[test]
fn bare_v0_requires_an_explicit_record_opt_in() {
    assert!(matches!(
        from_versioned_json::<MigratingRecord>(r#"{"value":"fixture","generation":2}"#),
        Err(VersionedRecordError::BareV0Unsupported { .. })
    ));
}
