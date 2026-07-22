use serde_json::{Value, json};
use starweaver_rpc_core::generated::{
    ERROR_CODE_INVALID_REQUEST, ERROR_CODE_METHOD_NOT_FOUND, ERROR_CODE_NOT_FOUND,
    ERROR_CODE_PARSE_ERROR, PROTOCOL_IDENTITY,
};

pub struct ConformanceVector {
    pub group: &'static str,
    pub method: &'static str,
    pub params: Value,
    pub expected_error: Option<i64>,
}

pub struct InvalidRequestVector {
    pub group: &'static str,
    pub body: &'static str,
    pub expected_code: i64,
    pub expected_id: Option<&'static str>,
}

pub const fn invalid_request_vectors() -> [InvalidRequestVector; 4] {
    [
        InvalidRequestVector {
            group: "malformed JSON",
            body: "{",
            expected_code: ERROR_CODE_PARSE_ERROR,
            expected_id: None,
        },
        InvalidRequestVector {
            group: "empty object",
            body: "{}",
            expected_code: ERROR_CODE_INVALID_REQUEST,
            expected_id: None,
        },
        InvalidRequestVector {
            group: "missing method with recoverable ID",
            body: r#"{"jsonrpc":"2.0","id":"req_missing_method","params":{}}"#,
            expected_code: ERROR_CODE_INVALID_REQUEST,
            expected_id: Some("req_missing_method"),
        },
        InvalidRequestVector {
            group: "non-string method with recoverable ID",
            body: r#"{"jsonrpc":"2.0","id":"req_non_string_method","method":7,"params":{}}"#,
            expected_code: ERROR_CODE_INVALID_REQUEST,
            expected_id: Some("req_non_string_method"),
        },
    ]
}

pub fn assert_invalid_request_response(vector: &InvalidRequestVector, response: &Value) {
    assert_eq!(response["jsonrpc"], "2.0", "group: {}", vector.group);
    assert_eq!(
        response["error"]["code"], vector.expected_code,
        "group: {}; response: {response}",
        vector.group
    );
    match vector.expected_id {
        Some(id) => assert_eq!(response["id"], id, "group: {}", vector.group),
        None => assert!(
            response["id"].is_null(),
            "group: {}; response: {response}",
            vector.group
        ),
    }
}

pub fn initialize_request(id: &str, client_name: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "initialize",
        "params": {
            "clientInfo": {
                "name": client_name,
                "version": env!("CARGO_PKG_VERSION")
            },
            "protocol": {
                "name": PROTOCOL_IDENTITY.name,
                "major": PROTOCOL_IDENTITY.major,
                "revision": PROTOCOL_IDENTITY.revision,
                "schemaDigest": PROTOCOL_IDENTITY.schema_digest
            },
            "requiredFeatures": [],
            "supportedFeatures": [
                "clarifications",
                "diagnostics.safe",
                "environment.attachments",
                "environment.mounts",
                "events.replay",
                "events.subscribe",
                "hitl",
                "host.shutdown",
                "profiles",
                "runs",
                "session.fork",
                "session.search",
                "sessions",
                "steering"
            ]
        }
    })
}

pub fn conformance_vectors() -> Vec<ConformanceVector> {
    vec![
        ConformanceVector {
            group: "diagnostics",
            method: "diagnostics.get",
            params: json!({}),
            expected_error: None,
        },
        ConformanceVector {
            group: "profiles",
            method: "catalog.list",
            params: json!({}),
            expected_error: None,
        },
        ConformanceVector {
            group: "sessions",
            method: "session.list",
            params: json!({"limit": 1}),
            expected_error: None,
        },
        ConformanceVector {
            group: "stable session not-found error",
            method: "session.get",
            params: json!({
                "sessionId": "missing_conformance_session",
                "runLimit": 1
            }),
            expected_error: Some(ERROR_CODE_NOT_FOUND),
        },
        ConformanceVector {
            group: "stable run not-found error",
            method: "run.status",
            params: json!({
                "sessionId": "missing_conformance_session",
                "runId": "missing_conformance_run"
            }),
            expected_error: Some(ERROR_CODE_NOT_FOUND),
        },
        ConformanceVector {
            group: "events",
            method: "events.replay",
            params: json!({
                "limit": 1,
                "view": {
                    "optionalFeatures": [],
                    "profile": "operations.v1",
                    "scope": {
                        "kind": "session",
                        "sessionId": "missing_conformance_session"
                    }
                }
            }),
            expected_error: None,
        },
        ConformanceVector {
            group: "approvals",
            method: "approval.list",
            params: json!({"limit": 1}),
            expected_error: None,
        },
        ConformanceVector {
            group: "deferred tools",
            method: "deferred.list",
            params: json!({"limit": 1}),
            expected_error: None,
        },
        ConformanceVector {
            group: "environments",
            method: "environment.list",
            params: json!({"limit": 1}),
            expected_error: None,
        },
        ConformanceVector {
            group: "method-not-found error",
            method: "missing.conformance_method",
            params: json!({}),
            expected_error: Some(ERROR_CODE_METHOD_NOT_FOUND),
        },
    ]
}

pub fn assert_conformance_response(vector: &ConformanceVector, response: &Value) {
    assert_eq!(response["jsonrpc"], "2.0", "group: {}", vector.group);
    match vector.expected_error {
        Some(code) => assert_eq!(
            response["error"]["code"], code,
            "group: {}; response: {response}",
            vector.group
        ),
        None => assert!(
            response.get("result").is_some(),
            "group: {}; response: {response}",
            vector.group
        ),
    }
}
