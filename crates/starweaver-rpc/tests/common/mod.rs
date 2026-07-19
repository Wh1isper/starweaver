use serde_json::{Value, json};
use starweaver_rpc_core::{INVALID_PARAMS, METHOD_NOT_FOUND, NOT_FOUND, UNSUPPORTED_FEATURE};

pub struct ConformanceVector {
    pub group: &'static str,
    pub method: &'static str,
    pub params: Value,
    pub expected_error: Option<i64>,
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
            method: "profile.list",
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
            params: json!({"sessionId": "missing-conformance-session"}),
            expected_error: Some(NOT_FOUND),
        },
        ConformanceVector {
            group: "runs",
            method: "run.status",
            params: json!({}),
            expected_error: Some(INVALID_PARAMS),
        },
        ConformanceVector {
            group: "streams",
            method: "stream.replay",
            params: json!({}),
            expected_error: Some(INVALID_PARAMS),
        },
        ConformanceVector {
            group: "hitl approvals",
            method: "approval.list",
            params: json!({}),
            expected_error: None,
        },
        ConformanceVector {
            group: "hitl deferred",
            method: "deferred.list",
            params: json!({}),
            expected_error: None,
        },
        ConformanceVector {
            group: "environments",
            method: "environment.list",
            params: json!({}),
            expected_error: None,
        },
        ConformanceVector {
            group: "unsupported feature error",
            method: "stream.subscribe",
            params: json!({}),
            expected_error: Some(UNSUPPORTED_FEATURE),
        },
        ConformanceVector {
            group: "method-not-found error",
            method: "missing.conformance_method",
            params: json!({}),
            expected_error: Some(METHOD_NOT_FOUND),
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
