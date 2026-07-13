#![allow(missing_docs, clippy::expect_used)]

use serde::Deserialize;
use serde_json::Value;
use starweaver_core::ProtocolIdentity;
use starweaver_rpc_core::{
    HostInitializeParams, INVALID_PARAMS, INVALID_REQUEST, handle_json_rpc_text,
    host_protocol_identity, validate_host_initialize,
};

const HOST_INITIALIZE: &str = include_str!("fixtures/contracts/host-initialize-v1.json");

#[derive(Deserialize)]
struct HostInitializeFixture {
    identity: ProtocolIdentity,
    legacy: HostInitializeParams,
    current: HostInitializeParams,
    wrong_name: HostInitializeParams,
    wrong_major: HostInitializeParams,
}

#[test]
fn host_identity_matches_the_release_fixture() {
    let fixture = serde_json::from_str::<HostInitializeFixture>(HOST_INITIALIZE)
        .expect("read host initialize fixture");
    assert_eq!(host_protocol_identity(), fixture.identity);
    let encoded = serde_json::to_value(host_protocol_identity()).expect("encode host identity");
    assert!(encoded.get("protocolVersion").is_none());
    assert!(encoded.get("protocol_version").is_none());
}

#[test]
fn host_initialize_accepts_legacy_and_matching_major_but_rejects_wrong_peers() {
    let fixture = serde_json::from_str::<HostInitializeFixture>(HOST_INITIALIZE)
        .expect("read host initialize fixture");
    validate_host_initialize(&fixture.legacy).expect("legacy initialize remains readable");
    validate_host_initialize(&fixture.current).expect("matching major is compatible");

    let wrong_name =
        validate_host_initialize(&fixture.wrong_name).expect_err("wrong protocol name must fail");
    assert_eq!(wrong_name.code, INVALID_PARAMS);
    assert!(
        wrong_name
            .message
            .contains("expected protocol starweaver.host")
    );

    let wrong_major =
        validate_host_initialize(&fixture.wrong_major).expect_err("wrong protocol major must fail");
    assert_eq!(wrong_major.code, INVALID_PARAMS);
    assert!(
        wrong_major
            .message
            .contains("unsupported starweaver.host major 2")
    );
}

#[test]
fn host_initialize_params_have_only_one_typed_version_identity() {
    let fixture = serde_json::from_str::<Value>(HOST_INITIALIZE).expect("parse fixture");
    let current = &fixture["current"];
    assert!(current.get("protocol").is_some());
    assert!(current.get("protocolVersion").is_none());
    assert!(current.get("protocol_version").is_none());
}

#[test]
fn json_rpc_envelope_normalizes_omitted_params_and_validates_ids() {
    let outcome = handle_json_rpc_text(
        r#"{"jsonrpc":"2.0","id":null,"method":"probe"}"#,
        |method, params| {
            assert_eq!(method, "probe");
            assert_eq!(params, &serde_json::json!({}));
            Ok(serde_json::json!({"ok": true}))
        },
    );
    let response = outcome.response.expect("explicit null id gets a response");
    assert!(response["id"].is_null());
    assert_eq!(response["result"]["ok"], true);

    let notification = handle_json_rpc_text(r#"{"jsonrpc":"2.0","method":"probe"}"#, |_, _| {
        Ok(serde_json::json!({}))
    });
    assert!(notification.response.is_none());

    for invalid in [
        r#"{"jsonrpc":"2.0","id":1.5,"method":"probe","params":{}}"#,
        r#"{"jsonrpc":"2.0","id":true,"method":"probe","params":{}}"#,
        r#"{"jsonrpc":"2.0","id":{},"method":"probe","params":{}}"#,
    ] {
        let response = handle_json_rpc_text(invalid, |_, _| unreachable!())
            .response
            .expect("invalid request response");
        assert_eq!(response["error"]["code"], INVALID_REQUEST);
        assert!(response["id"].is_null());
    }
}

#[test]
fn json_rpc_envelope_rejects_non_object_params_before_dispatch() {
    for params in ["null", "true", "1", r#""scalar""#, "[]", "[1]"] {
        let request =
            format!(r#"{{"jsonrpc":"2.0","id":"bad-params","method":"probe","params":{params}}}"#);
        let response = handle_json_rpc_text(&request, |_, _| unreachable!())
            .response
            .expect("invalid request response");
        assert_eq!(response["error"]["code"], INVALID_REQUEST);
        assert_eq!(response["id"], "bad-params");
    }
}
