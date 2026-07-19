#![allow(clippy::expect_used, clippy::too_many_lines, missing_docs)]

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use starweaver_core::{RunId, SessionId};
use starweaver_rpc_core::*;
use starweaver_stream::{
    DisplayMessage, DisplayMessageKind, ReplayCursor, ReplayEvent, ReplayScope,
};

const RPC_CONTRACT_CATALOG: &str = include_str!("fixtures/contracts/rpc-contract-catalog-v1.json");
const RPC_WIRE: &str = include_str!("fixtures/contracts/rpc-wire-v1.json");
const RPC_WIRE_SCHEMA: &str = include_str!("../schemas/rpc-wire-v1.schema.json");

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConformanceCorpus {
    schema: String,
    major: u32,
    revision: String,
    methods: Vec<MethodFixture>,
    notifications: Vec<String>,
}

#[derive(Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
struct MethodFixture {
    method: String,
    params_type: String,
    result_type: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WireCorpus {
    schema: String,
    major: u32,
    revision: String,
    methods: Vec<WireMethodFixture>,
    notifications: Vec<Value>,
    invalid_notifications: Vec<Value>,
    errors: Vec<WireErrorFixture>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WireMethodFixture {
    method: String,
    canonical_params: Value,
    canonical_result: Value,
    invalid_params: Vec<Value>,
    invalid_results: Vec<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WireErrorFixture {
    name: String,
    code: i64,
    response: Value,
    invalid_responses: Vec<Value>,
}

#[test]
fn rpc_catalog_covers_every_v1_method_and_notification_exactly() {
    let fixture = serde_json::from_str::<ConformanceCorpus>(RPC_CONTRACT_CATALOG)
        .expect("parse RPC contract catalog");
    assert_eq!(fixture.schema, "starweaver.host.contract-catalog");
    assert_eq!(fixture.major, HOST_PROTOCOL_MAJOR);
    assert_eq!(fixture.revision, "2026-07-18");

    let expected = V1_METHOD_CONTRACTS
        .iter()
        .map(|contract| MethodFixture {
            method: contract.name.to_string(),
            params_type: contract.params.to_string(),
            result_type: contract.result.to_string(),
        })
        .collect::<Vec<_>>();
    assert_eq!(fixture.methods, expected);
    assert_eq!(
        fixture.notifications,
        V1_NOTIFICATION_METHODS
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
    );

    let methods = fixture
        .methods
        .iter()
        .map(|entry| entry.method.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(methods.len(), fixture.methods.len(), "duplicate RPC method");
    assert!(methods.contains("storage.importLegacy"));
    assert!(methods.contains("environment.active_mount"));
    assert!(!methods.contains("storage.import_legacy"));
    assert!(!methods.contains("environment.activeMount"));
}

#[test]
fn catalog_and_runtime_dto_validators_share_exact_method_names() {
    for contract in V1_METHOD_CONTRACTS {
        for validation in [
            validate_v1_method_params(contract.name, &Value::Null),
            validate_v1_method_result(contract.name, &Value::Null),
        ] {
            if let Err(error) = validation {
                assert!(
                    !error.starts_with("unknown v1 method:"),
                    "catalog method {} is absent from a DTO validator",
                    contract.name
                );
            }
        }
    }

    assert_eq!(
        validate_v1_method_params("storage.import_legacy", &json!({})),
        Err("unknown v1 method: storage.import_legacy".to_string())
    );
    assert_eq!(
        validate_v1_method_result("environment.activeMount", &json!({})),
        Err("unknown v1 method: environment.activeMount".to_string())
    );
}

const fn serde_contract<P, R>()
where
    P: DeserializeOwned + Serialize,
    R: DeserializeOwned + Serialize,
{
}

#[test]
fn every_catalog_entry_names_concrete_serde_dtos() {
    serde_contract::<HostInitializeParams, HostInitializeResult>();
    serde_contract::<EmptyParams, ShutdownResult>();
    serde_contract::<EmptyParams, DiagnosticsGetResult>();
    serde_contract::<ClientStateParams, ProfileListResult>();
    serde_contract::<ClientStateParams, ModelListResult>();
    serde_contract::<ProfileGetParams, ProfileGetResult>();
    serde_contract::<ClientStateParams, ModelCurrentResult>();
    serde_contract::<ModelSelectParams, ModelSelectResult>();
    serde_contract::<ConfigGetParams, ConfigGetResult>();
    serde_contract::<StorageImportLegacyParams, StorageImportLegacyResult>();
    serde_contract::<SessionCreateParams, SessionCreateResult>();
    serde_contract::<SessionListParams, SessionListResult>();
    serde_contract::<SessionSearchParams, SessionSearchResult>();
    serde_contract::<SessionGetParams, SessionGetResult>();
    serde_contract::<EmptyParams, SessionCurrentResult>();
    serde_contract::<SessionCurrentSetParams, SessionCurrentResult>();
    serde_contract::<SessionDeleteParams, SessionDeleteResult>();
    serde_contract::<RunStartParams, RunStartResult>();
    serde_contract::<RunResumeParams, RunResumeResult>();
    serde_contract::<RunPromptParams, RunPromptResult>();
    serde_contract::<RunIdentityParams, RunStatusResult>();
    serde_contract::<RunAwaitParams, RunAwaitResult>();
    serde_contract::<RunCancelParams, RunCancelResult>();
    serde_contract::<RunSteerParams, RunSteerResult>();
    serde_contract::<RunAttachParams, RunAttachmentResult>();
    serde_contract::<SessionOutputParams, SessionOutputResult>();
    serde_contract::<StreamReplayParams, StreamReplayResult>();
    serde_contract::<SessionReplayParams, SessionReplayResult>();
    serde_contract::<ApprovalListParams, ApprovalListResult>();
    serde_contract::<ApprovalShowParams, ApprovalShowResult>();
    serde_contract::<ApprovalDecideParams, ApprovalDecideResult>();
    serde_contract::<DeferredListParams, DeferredListResult>();
    serde_contract::<DeferredShowParams, DeferredShowResult>();
    serde_contract::<DeferredCompleteParams, DeferredCompleteResult>();
    serde_contract::<DeferredFailParams, DeferredFailResult>();
    serde_contract::<EnvironmentAttachParams, EnvironmentAttachResult>();
    serde_contract::<EnvironmentDetachParams, EnvironmentDetachResult>();
    serde_contract::<EnvironmentListParams, EnvironmentListResult>();
    serde_contract::<EnvironmentHealthParams, EnvironmentHealthResult>();
    serde_contract::<EnvironmentActiveMountParams, EnvironmentActiveMountResult>();
    serde_contract::<EnvironmentActiveUnmountParams, EnvironmentActiveUnmountResult>();
    serde_contract::<EnvironmentActiveListParams, EnvironmentActiveListResult>();
    serde_contract::<StreamSubscribeParams, StreamSubscribeResult>();
    serde_contract::<StreamUnsubscribeParams, StreamUnsubscribeResult>();
}

#[test]
fn typed_notification_union_locks_methods_and_camel_case_payloads() {
    let scope = ReplayScope::run("run_fixture");
    let cursor = ReplayCursor::replay_event(scope.clone(), 4);
    let ready = typed_notification(HostNotificationKind::SubscriptionReady(
        SubscriptionReadyParams {
            subscription_id: "sub_fixture".to_string(),
            scope: scope.clone(),
            cursor: Some(cursor.clone()),
        },
    ));
    assert_eq!(ready["jsonrpc"], "2.0");
    assert_eq!(ready["method"], "subscription.ready");
    assert_eq!(ready["params"]["subscriptionId"], "sub_fixture");
    serde_json::from_value::<HostNotification>(ready).expect("parse ready notification");

    let status = typed_notification(HostNotificationKind::RunStatus(HostRunStatus {
        session_id: SessionId::from_string("session_fixture"),
        run_id: RunId::from_string("run_fixture"),
        status: "completed".to_string(),
        output_preview: Some("done".to_string()),
        error: None,
        continuation_effect: None,
    }));
    assert_eq!(status["method"], "run.status");
    assert_eq!(status["params"]["outputPreview"], "done");
    serde_json::from_value::<HostNotification>(status).expect("parse status notification");

    let diagnostic = typed_notification(HostNotificationKind::Diagnostic(
        DiagnosticNotificationParams {
            level: DiagnosticLevel::Error,
            message: "fixture failure".to_string(),
            subscription_id: Some("sub_fixture".to_string()),
            code: Some("replay_failed".to_string()),
        },
    ));
    assert_eq!(diagnostic["method"], "diagnostic");
    assert_eq!(diagnostic["params"]["subscriptionId"], "sub_fixture");
    serde_json::from_value::<HostNotification>(diagnostic).expect("parse diagnostic notification");

    let mut message = DisplayMessage::new(
        4,
        SessionId::from_string("session_fixture"),
        RunId::from_string("run_fixture"),
        DisplayMessageKind::RunStarted,
    );
    message.payload = json!({"status": "running"});
    let event = ReplayEvent::display_at(scope.clone(), 4, message);
    let item = output_item(&event, StreamPayloadFormat::DisplayMessage)
        .expect("display event has output projection");
    let stream = typed_notification(HostNotificationKind::StreamEvent(Box::new(
        StreamEventParams {
            subscription_id: "sub_fixture".to_string(),
            scope: scope.clone(),
            cursor,
            item,
        },
    )));
    assert_eq!(stream["method"], "stream.event");
    assert_eq!(stream["params"]["item"]["payloadFormat"], "display_message");
    serde_json::from_value::<HostNotification>(stream).expect("parse stream notification");

    let closed = typed_notification(HostNotificationKind::SubscriptionClosed(
        SubscriptionClosedParams {
            subscription_id: "sub_fixture".to_string(),
            scope,
            reason: SubscriptionClosedReason::Terminal,
        },
    ));
    assert_eq!(closed["method"], "subscription.closed");
    assert_eq!(closed["params"]["reason"], "terminal");
    serde_json::from_value::<HostNotification>(closed).expect("parse closed notification");
}

#[test]
fn run_materialization_wire_contract_uses_defaults_and_camel_case() {
    let start = serde_json::from_value::<RunStartParams>(json!({
        "prompt": "hello"
    }))
    .expect("default continuation mode");
    assert_eq!(start.continuation_mode, ContinuationMode::Preserve);

    let resume = serde_json::from_value::<RunResumeParams>(json!({
        "sessionId": "session_fixture",
        "runId": "run_fixture",
        "idempotencyKey": "resume_fixture"
    }))
    .expect("default resume continuation mode");
    assert_eq!(resume.continuation_mode, ContinuationMode::Preserve);

    let materialization = AgentMaterialization {
        version: 2,
        agent_spec_digest: "sha256:spec".to_string(),
        model_profile_id: "general".to_string(),
        toolset_ids: vec!["filesystem".to_string()],
        policy_version: "starweaver-agent-policy-v1".to_string(),
        environment_binding_class: "local:read_write".to_string(),
        runtime_binding_digest: "sha256:runtime".to_string(),
        workspace_root_digest: "sha256:workspace".to_string(),
        fingerprint: "sha256:fingerprint".to_string(),
    };
    let result = RunStartResult {
        session_id: SessionId::from_string("session_fixture"),
        run_id: RunId::from_string("run_fixture"),
        status: "running".to_string(),
        idempotent_replay: false,
        payload_format: "display".to_string(),
        environment_attachments: Vec::new(),
        materialization: Some(materialization),
        continuation: Some(ContinuationAssessment {
            mode: ContinuationMode::Switch,
            source_fingerprint: None,
            target_fingerprint: "sha256:fingerprint".to_string(),
            drift: vec![MaterializationDrift {
                field: "sourceEvidence".to_string(),
                source: None,
                target: json!("verified"),
            }],
            allowed: true,
        }),
    };
    let encoded = serde_json::to_value(&result).expect("serialize run.start result");
    assert_eq!(encoded["materialization"]["modelProfileId"], "general");
    assert_eq!(encoded["continuation"]["mode"], "switch");
    assert!(encoded["materialization"].get("model_profile_id").is_none());
    assert_eq!(
        serde_json::from_value::<RunStartResult>(encoded).expect("roundtrip run.start result"),
        result
    );
}

#[test]
fn canonical_run_and_subscription_params_reject_wrong_casing() {
    let start = serde_json::from_value::<RunStartParams>(json!({
        "prompt": "hello",
        "sessionId": "session_fixture",
        "restoreFromRunId": "run_parent",
        "clientStateScope": "host_fixture",
        "environmentAttachments": []
    }))
    .expect("canonical run.start params");
    assert_eq!(
        start.session_id.expect("session id").as_str(),
        "session_fixture"
    );

    let wrong = serde_json::from_value::<RunStartParams>(json!({
        "prompt": "hello",
        "session_id": "session_fixture"
    }));
    assert!(wrong.is_err());

    let subscribe = serde_json::from_value::<StreamSubscribeParams>(json!({
        "sessionId": "session_fixture",
        "runId": "run_fixture",
        "subscriptionId": "sub_fixture",
        "payloadFormat": "display_message",
        "limit": 100
    }))
    .expect("canonical stream.subscribe params");
    assert_eq!(subscribe.limit, Some(100));
    assert_eq!(
        subscribe.payload_format,
        StreamPayloadFormat::DisplayMessage
    );

    let value = serde_json::to_value(subscribe).expect("serialize subscription params");
    assert!(value.get("subscriptionId").is_some());
    assert!(value.get("subscription_id").is_none());
    assert_eq!(
        value.get("payloadFormat"),
        Some(&Value::String("display_message".into()))
    );
}

#[test]
fn concrete_rpc_wire_corpus_is_complete_and_rust_conformant() {
    let corpus = serde_json::from_str::<WireCorpus>(RPC_WIRE).expect("parse wire corpus");
    assert_eq!(corpus.schema, "starweaver.host.wire-corpus");
    assert_eq!(corpus.major, HOST_PROTOCOL_MAJOR);
    assert_eq!(corpus.revision, "2026-07-19");
    assert_eq!(corpus.methods.len(), V1_METHOD_CONTRACTS.len());

    for (fixture, contract) in corpus.methods.iter().zip(V1_METHOD_CONTRACTS) {
        assert_eq!(fixture.method, contract.name);
        validate_v1_method_params(&fixture.method, &fixture.canonical_params).unwrap_or_else(
            |error| {
                panic!(
                    "canonical {} params failed Rust DTO validation: {error}",
                    fixture.method
                )
            },
        );
        validate_v1_method_result(&fixture.method, &fixture.canonical_result).unwrap_or_else(
            |error| {
                panic!(
                    "canonical {} result failed Rust DTO validation: {error}",
                    fixture.method
                )
            },
        );
        assert!(
            !fixture.invalid_params.is_empty(),
            "{} must have an invalid params vector",
            fixture.method
        );
        for invalid in &fixture.invalid_params {
            assert!(
                validate_v1_method_params(&fixture.method, invalid).is_err(),
                "invalid {} params unexpectedly matched its Rust DTO: {invalid}",
                fixture.method
            );
        }
        assert!(
            !fixture.invalid_results.is_empty(),
            "{} must have an invalid result vector",
            fixture.method
        );
        for invalid in &fixture.invalid_results {
            assert!(
                validate_v1_method_result(&fixture.method, invalid).is_err(),
                "invalid {} result unexpectedly matched its Rust DTO: {invalid}",
                fixture.method
            );
        }
    }
}

#[test]
fn concrete_notification_and_error_corpus_covers_stable_wire_surface() {
    let corpus = serde_json::from_str::<WireCorpus>(RPC_WIRE).expect("parse wire corpus");
    assert_eq!(
        corpus.invalid_notifications.len(),
        corpus.notifications.len()
    );
    let notification_methods = corpus
        .notifications
        .iter()
        .map(|notification| {
            assert_eq!(notification["jsonrpc"], "2.0");
            validate_v1_notification(notification)
                .expect("notification must match typed Rust union");
            notification["method"]
                .as_str()
                .expect("notification method")
                .to_string()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        notification_methods,
        V1_NOTIFICATION_METHODS
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
    );

    for invalid in &corpus.invalid_notifications {
        assert!(
            validate_v1_notification(invalid).is_err(),
            "invalid notification unexpectedly matched the typed union: {invalid}"
        );
    }

    let errors = corpus
        .errors
        .iter()
        .map(|fixture| {
            assert_eq!(fixture.response["jsonrpc"], "2.0");
            assert_eq!(fixture.response["id"], "error_fixture");
            assert_eq!(fixture.response["error"]["code"], fixture.code);
            assert!(fixture.response["error"]["message"].is_string());
            validate_v1_error_response(fixture.code, &fixture.response)
                .expect("canonical error response must decode strictly");
            assert!(!fixture.invalid_responses.is_empty());
            for invalid in &fixture.invalid_responses {
                assert!(
                    validate_v1_error_response(fixture.code, invalid).is_err(),
                    "invalid {} error response unexpectedly decoded: {invalid}",
                    fixture.name
                );
            }
            (fixture.name.as_str(), fixture.code)
        })
        .collect::<Vec<_>>();
    let expected = V1_ERROR_CONTRACTS
        .iter()
        .map(|contract| (contract.name, contract.code))
        .collect::<Vec<_>>();
    assert_eq!(errors, expected);
}

#[test]
fn the_same_wire_corpus_passes_in_process_dispatch() {
    let corpus = serde_json::from_str::<WireCorpus>(RPC_WIRE).expect("parse wire corpus");
    for (index, fixture) in corpus.methods.iter().enumerate() {
        let id = u64::try_from(index + 1).expect("fixture id");
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": fixture.method,
            "params": fixture.canonical_params,
        });
        let outcome = handle_json_rpc_text(&request.to_string(), |method, params| {
            assert_eq!(method, fixture.method);
            assert_eq!(params, &fixture.canonical_params);
            Ok(fixture.canonical_result.clone())
        });
        let response = outcome.response.expect("canonical request response");
        assert_eq!(response["id"], id);
        assert_eq!(response["result"], fixture.canonical_result);

        for invalid in &fixture.invalid_params {
            let request = json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": fixture.method,
                "params": invalid,
            });
            let outcome = handle_json_rpc_text(&request.to_string(), |_method, _params| {
                panic!("invalid typed params must be rejected before dispatch")
            });
            assert_eq!(
                outcome.response.expect("invalid params response")["error"]["code"],
                INVALID_PARAMS,
                "{} invalid vector did not fail at the in-process boundary: {invalid}",
                fixture.method
            );
        }

        for invalid in &fixture.invalid_results {
            let request = json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": fixture.method,
                "params": fixture.canonical_params,
            });
            let outcome =
                handle_json_rpc_text(&request.to_string(), |_method, _params| Ok(invalid.clone()));
            assert_eq!(
                outcome.response.expect("invalid result response")["error"]["code"],
                SERVER_ERROR,
                "{} invalid result did not fail at the in-process boundary: {invalid}",
                fixture.method
            );
        }
    }
}

#[test]
fn generated_rpc_schema_accepts_only_the_current_corpus_shape() {
    let schema = serde_json::from_str::<Value>(RPC_WIRE_SCHEMA).expect("parse generated schema");
    let validator = jsonschema::validator_for(&schema).expect("compile generated schema");
    let corpus = serde_json::from_str::<Value>(RPC_WIRE).expect("parse wire corpus value");
    assert!(validator.is_valid(&corpus));

    let mut stale = corpus.clone();
    stale["methods"][0]["method"] = json!("initialize.stale");
    assert!(!validator.is_valid(&stale));

    let mut malformed = corpus;
    malformed["methods"][0]["canonicalParams"] = json!({"protocol": 1});
    assert!(!validator.is_valid(&malformed));
}
