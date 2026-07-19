#![allow(clippy::expect_used, clippy::too_many_lines, missing_docs)]

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use starweaver_core::{RunId, SessionId};
use starweaver_rpc_core::*;
use starweaver_stream::{
    DisplayMessage, DisplayMessageKind, ReplayCursor, ReplayEvent, ReplayScope,
};

const DESKTOP_CONFORMANCE: &str = include_str!("fixtures/contracts/desktop-conformance-v1.json");

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

#[test]
fn desktop_corpus_covers_every_v1_method_and_notification_exactly() {
    let fixture = serde_json::from_str::<ConformanceCorpus>(DESKTOP_CONFORMANCE)
        .expect("parse Desktop conformance corpus");
    assert_eq!(fixture.schema, "starweaver.host.desktop-conformance");
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
fn canonical_run_and_subscription_params_reject_wrong_casing() {
    let start = serde_json::from_value::<RunStartParams>(json!({
        "prompt": "hello",
        "sessionId": "session_fixture",
        "restoreFromRunId": "run_parent",
        "clientStateScope": "desktop",
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
