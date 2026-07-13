#![allow(missing_docs, clippy::expect_used)]

use serde::Deserialize;
use serde_json::Value;
use starweaver_core::ProtocolIdentity;
use starweaver_envd_core::{
    EnvdErrorCode, InitializeEnvdRequest, InitializeEnvdResult, envd_protocol_identity,
    validate_envd_initialize, validate_envd_protocol,
};

const ENVD_INITIALIZE: &str = include_str!("fixtures/contracts/envd-initialize-v1.json");

#[derive(Deserialize)]
struct EnvdInitializeFixture {
    identity: ProtocolIdentity,
    legacy_request: InitializeEnvdRequest,
    current_request: InitializeEnvdRequest,
    current_result: InitializeEnvdResult,
    wrong_name: InitializeEnvdRequest,
    wrong_major: InitializeEnvdRequest,
}

#[test]
fn envd_identity_and_initialize_result_match_the_release_fixture() {
    let fixture = serde_json::from_str::<EnvdInitializeFixture>(ENVD_INITIALIZE)
        .expect("read envd initialize fixture");
    assert_eq!(envd_protocol_identity(), fixture.identity);
    assert_eq!(fixture.current_result.protocol, fixture.identity);
    validate_envd_protocol(&fixture.current_result.protocol).expect("validate result identity");

    let encoded = serde_json::to_value(&fixture.current_result).expect("encode envd result");
    assert!(encoded.get("protocol").is_some());
    assert!(encoded.get("protocolVersion").is_none());
    assert!(encoded.get("protocol_version").is_none());
}

#[test]
fn envd_initialize_accepts_legacy_and_matching_major_but_rejects_wrong_peers() {
    let fixture = serde_json::from_str::<EnvdInitializeFixture>(ENVD_INITIALIZE)
        .expect("read envd initialize fixture");
    validate_envd_initialize(&fixture.legacy_request).expect("legacy initialize remains readable");
    validate_envd_initialize(&fixture.current_request).expect("matching major is compatible");

    let wrong_name =
        validate_envd_initialize(&fixture.wrong_name).expect_err("wrong protocol name must fail");
    assert_eq!(wrong_name.code, EnvdErrorCode::InvalidRequest);
    assert!(
        wrong_name
            .message
            .contains("expected protocol starweaver.envd")
    );

    let wrong_major =
        validate_envd_initialize(&fixture.wrong_major).expect_err("wrong protocol major must fail");
    assert_eq!(wrong_major.code, EnvdErrorCode::InvalidRequest);
    assert!(
        wrong_major
            .message
            .contains("unsupported starweaver.envd major 2")
    );
}

#[test]
fn envd_initialize_fixture_has_only_one_typed_version_identity() {
    let fixture = serde_json::from_str::<Value>(ENVD_INITIALIZE).expect("parse fixture");
    let result = &fixture["current_result"];
    assert!(result.get("protocol").is_some());
    assert!(result.get("protocolVersion").is_none());
    assert!(result.get("protocol_version").is_none());
}
