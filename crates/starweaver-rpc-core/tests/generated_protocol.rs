#![allow(missing_docs)]

use starweaver_rpc_core::generated::{
    DecimalU64, DecodeServerFrameError, EventClass, EventProfile, HostCall, HostError,
    HostErrorData, HostNotification, HostNotificationParams, HostResult, HostServerFrame,
    InitializeError, InvalidParamsData, InvalidParamsDataKind, Method, NotFoundData,
    NotFoundDataKind, Notification, PROTOCOL_MAJOR, PROTOCOL_NAME, PROTOCOL_REVISION, RequestId,
    SCHEMA_DIGEST, SubscriptionClosedNotificationParams, SubscriptionClosedReason, SubscriptionId,
    decode_request_frame, decode_server_frame, encode_error_response_frame,
    encode_notification_frame, encode_request_frame,
};

#[test]
fn generated_identity_is_the_single_major_one_contract() {
    assert_eq!(PROTOCOL_NAME, "starweaver.host");
    assert_eq!(PROTOCOL_MAJOR, 1);
}

#[test]
fn request_ids_are_non_empty_strings_and_params_are_required_objects() {
    let Ok(request) = decode_request_frame(
        br#"{"jsonrpc":"2.0","id":"req_1","method":"diagnostics.get","params":{}}"#,
    ) else {
        panic!("canonical request must decode");
    };
    assert_eq!(request.id.as_str(), "req_1");
    assert!(matches!(request.call, HostCall::DiagnosticsGet(_)));

    for invalid in [
        br#"{"jsonrpc":"2.0","id":1,"method":"diagnostics.get","params":{}}"#.as_slice(),
        br#"{"jsonrpc":"2.0","id":"","method":"diagnostics.get","params":{}}"#,
        br#"{"jsonrpc":"2.0","id":"req","method":"diagnostics.get"}"#,
        br#"{"jsonrpc":"2.0","id":"req","method":"diagnostics.get","params":null}"#,
        br#"[{"jsonrpc":"2.0","id":"req","method":"diagnostics.get","params":{}}]"#,
    ] {
        assert!(decode_request_frame(invalid).is_err());
    }
}

#[test]
fn generated_client_encodes_requests_with_an_inseparable_correlation() {
    let Ok(request) = decode_request_frame(
        br#"{"jsonrpc":"2.0","id":"req_client","method":"diagnostics.get","params":{}}"#,
    ) else {
        panic!("canonical request must decode");
    };
    let Ok(encoded) = encode_request_frame(&request) else {
        panic!("typed request must encode");
    };
    assert_eq!(encoded.correlation.id.as_str(), "req_client");
    assert_eq!(encoded.correlation.method, Method::DiagnosticsGet);
    let Ok(round_trip) = decode_request_frame(&encoded.bytes) else {
        panic!("client-encoded request must satisfy the server decoder");
    };
    assert_eq!(round_trip, request);
}

#[test]
fn generated_client_strictly_correlates_typed_results_and_remote_errors() {
    let response = format!(
        r#"{{"jsonrpc":"2.0","id":"req_init","result":{{"launch":{{"acceptedMaximumVersion":1,"acceptedMinimumVersion":1,"configurationGeneration":"0","effectiveSchema":{{"name":"starweaver.rpc.launch","version":1}},"envelopeDigest":"sha256:0000000000000000000000000000000000000000000000000000000000000000","mode":"standalone"}},"negotiatedFeatures":[],"protocol":{{"major":{PROTOCOL_MAJOR},"name":"{PROTOCOL_NAME}","revision":"{PROTOCOL_REVISION}","schemaDigest":"{SCHEMA_DIGEST}"}},"runtimeBuild":{{"buildRevision":"source","target":"test-target","version":"0.9.0"}},"runtimeStatus":"ready","serverInfo":{{"name":"starweaver-rpc","version":"0.9.0"}},"startupReconciliation":{{"changedRunState":false,"repairedRuns":"0"}},"storage":{{"currentGeneration":"1","maintenanceBarrierGeneration":"0","maximumReadableGeneration":"1","maximumWritableGeneration":"1","minimumReadableGeneration":"1","minimumWritableGeneration":"1"}},"supportedFeatures":[],"workspace":{{"executionDomainId":"standalone-local","workspaceIdentity":"workspace-test"}}}}}}"#,
    );
    let Ok(frame) = decode_server_frame(response.as_bytes(), |id| {
        (id.as_str() == "req_init").then_some(Method::Initialize)
    }) else {
        panic!("valid initialize response must decode");
    };
    assert!(matches!(
        frame,
        HostServerFrame::Response(response)
            if response.correlation.method == Method::Initialize
                && matches!(response.result, Ok(HostResult::Initialize(_)))
    ));

    let error = br#"{"jsonrpc":"2.0","id":"req_init","error":{"code":-32602,"message":"invalid params","data":{"kind":"invalid_params","retryable":false,"reconciliationRequired":false}}}"#;
    let Ok(frame) = decode_server_frame(error, |_| Some(Method::Initialize)) else {
        panic!("declared initialize error must decode");
    };
    assert!(matches!(
        frame,
        HostServerFrame::Response(response) if response.result.is_err()
    ));

    let undeclared = br#"{"jsonrpc":"2.0","id":"req_init","error":{"code":-32010,"message":"not found","data":{"kind":"not_found","retryable":false,"reconciliationRequired":false}}}"#;
    assert_eq!(
        decode_server_frame(undeclared, |_| Some(Method::Initialize)),
        Err(DecodeServerFrameError::InvalidRemoteError)
    );
    let mismatched = br#"{"jsonrpc":"2.0","id":"req_init","error":{"code":-32010,"message":"wrong code","data":{"kind":"invalid_params","retryable":false,"reconciliationRequired":false}}}"#;
    assert_eq!(
        decode_server_frame(mismatched, |_| Some(Method::Initialize)),
        Err(DecodeServerFrameError::InvalidRemoteError)
    );
}

#[test]
fn generated_client_rejects_uncorrelated_or_non_strict_server_frames() {
    let response = br#"{"jsonrpc":"2.0","id":"unknown","result":{}}"#;
    assert_eq!(
        decode_server_frame(response, |_| None),
        Err(DecodeServerFrameError::UncorrelatedResponse)
    );
    for invalid in [
        br#"{"jsonrpc":"2.0","id":null,"result":{}}"#.as_slice(),
        br#"{"jsonrpc":"2.0","id":7,"result":{}}"#,
        br#"{"jsonrpc":"2.0","id":"req","result":{},"extra":true}"#,
        br#"[{"jsonrpc":"2.0","id":"req","result":{}}]"#,
    ] {
        assert!(decode_server_frame(invalid, |_| Some(Method::DiagnosticsGet)).is_err());
    }
}

#[test]
fn decode_errors_retain_only_valid_request_ids_and_encode_null_otherwise() {
    for (frame, expected_code) in [
        (
            br#"{"jsonrpc":"2.0","id":"req_method","method":"removed.method","params":{}}"#.as_slice(),
            -32601,
        ),
        (
            br#"{"jsonrpc":"2.0","id":"req_params","method":"diagnostics.get","params":{"unexpected":true}}"#,
            -32602,
        ),
    ] {
        let Err(error) = decode_request_frame(frame) else {
            panic!("invalid request must fail decoding");
        };
        assert!(error.id.is_some());
        assert_eq!(error.error.code, expected_code);
        let Ok(encoded) = encode_error_response_frame(&error.into_response()) else {
            panic!("typed decode error must encode");
        };
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&encoded) else {
            panic!("encoded error response must be JSON");
        };
        assert!(value["id"].as_str().is_some_and(|id| id.starts_with("req_")));
        assert_eq!(value["error"]["code"], expected_code);
    }

    for frame in [
        br#"{"jsonrpc":"2.0","id":"unterminated""#.as_slice(),
        br#"{"jsonrpc":"2.0","id":7,"method":"diagnostics.get","params":{}}"#,
        br"[]",
    ] {
        let Err(error) = decode_request_frame(frame) else {
            panic!("request without a recoverable canonical ID must fail");
        };
        assert!(error.id.is_none());
        let Ok(encoded) = encode_error_response_frame(&error.into_response()) else {
            panic!("typed decode error must encode");
        };
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&encoded) else {
            panic!("encoded error response must be JSON");
        };
        assert!(value["id"].is_null());
    }
}

#[test]
fn decoder_enforces_inline_ranges_lengths_and_unique_arrays() {
    for invalid in [
        br#"{"jsonrpc":"2.0","id":"req_limit","method":"session.list","params":{"limit":0}}"#.as_slice(),
        br#"{"jsonrpc":"2.0","id":"req_shutdown","method":"shutdown","params":{"deadlineMs":0}}"#.as_slice(),
        br#"{"jsonrpc":"2.0","id":"req_decision","method":"approval.decide","params":{"approvalId":"approval_1","decision":"other","expectedRevision":"1","idempotencyKey":"decision-1"}}"#.as_slice(),
        br#"{"jsonrpc":"2.0","id":"req_answer","method":"clarification.resolve","params":{"answers":[{"question":"Database?","selectedOptions":["same","same"]}],"clarificationId":"clarification_1","expectedRevision":"1","idempotencyKey":"answer-1","response":null}}"#.as_slice(),
    ] {
        let Err(error) = decode_request_frame(invalid) else {
            panic!("schema-invalid request must be rejected");
        };
        assert_eq!(error.error.code, -32602);
    }

    let duplicate_features = format!(
        r#"{{"jsonrpc":"2.0","id":"req_init","method":"initialize","params":{{"clientInfo":{{"name":"client","version":"1"}},"protocol":{{"major":{PROTOCOL_MAJOR},"name":"starweaver.host","revision":"{PROTOCOL_REVISION}","schemaDigest":"{SCHEMA_DIGEST}"}},"requiredFeatures":[],"supportedFeatures":["sessions","sessions"]}}}}"#,
    );
    let Err(error) = decode_request_frame(duplicate_features.as_bytes()) else {
        panic!("duplicate feature declarations must be rejected");
    };
    assert_eq!(error.error.code, -32602);

    for (required_features, supported_features, message) in [
        (
            "[]",
            r#"["sessions","runs"]"#,
            "unsorted supported feature declarations",
        ),
        (
            r#"["sessions","runs"]"#,
            r#"["runs","sessions"]"#,
            "unsorted required feature declarations",
        ),
        ("[]", r#"["Runs"]"#, "invalid feature identifiers"),
    ] {
        let invalid = format!(
            r#"{{"jsonrpc":"2.0","id":"req_init","method":"initialize","params":{{"clientInfo":{{"name":"client","version":"1"}},"protocol":{{"major":{PROTOCOL_MAJOR},"name":"starweaver.host","revision":"{PROTOCOL_REVISION}","schemaDigest":"{SCHEMA_DIGEST}"}},"requiredFeatures":{required_features},"supportedFeatures":{supported_features}}}}}"#,
        );
        let Err(error) = decode_request_frame(invalid.as_bytes()) else {
            panic!("{message} must be rejected");
        };
        assert_eq!(error.error.code, -32602);
    }
}

#[test]
fn deferred_tool_schema_is_complete_json_but_remains_object_only() {
    let valid = br#"{"jsonrpc":"2.0","id":"req_schema","method":"session.create","params":{"deferredTools":[{"description":"Render","inputSchema":{"type":"object","properties":{"title":{"type":"string"}}},"inputSchemaDigest":"sha256:0000000000000000000000000000000000000000000000000000000000000000","instructions":[],"name":"render"}],"idempotencyKey":"create-schema"}}"#;
    let Ok(request) = decode_request_frame(valid) else {
        panic!("complete object input schema must decode");
    };
    let HostCall::SessionCreate(params) = request.call else {
        panic!("session.create call expected");
    };
    assert!(params.deferred_tools[0].input_schema.is_object());

    let invalid = br#"{"jsonrpc":"2.0","id":"req_schema","method":"session.create","params":{"deferredTools":[{"description":"Render","inputSchema":[],"inputSchemaDigest":"sha256:0000000000000000000000000000000000000000000000000000000000000000","instructions":[],"name":"render"}],"idempotencyKey":"create-schema"}}"#;
    assert!(decode_request_frame(invalid).is_err());
}

#[test]
fn clarification_answers_are_closed_typed_and_revision_fenced() {
    let valid = br#"{"jsonrpc":"2.0","id":"req_answer","method":"clarification.resolve","params":{"answers":[{"question":"Which database?","selectedOptions":["PostgreSQL"],"freeText":null}],"clarificationId":"clarification_1","expectedRevision":"2","idempotencyKey":"answer-1","response":null}}"#;
    let Ok(request) = decode_request_frame(valid) else {
        panic!("typed clarification answer must decode");
    };
    let HostCall::ClarificationResolve(params) = request.call else {
        panic!("clarification.resolve call expected");
    };
    assert_eq!(params.expected_revision.get(), 2);
    assert_eq!(params.answers[0].selected_options, ["PostgreSQL"]);

    let missing_revision = br#"{"jsonrpc":"2.0","id":"req_answer","method":"clarification.resolve","params":{"answers":[],"clarificationId":"clarification_1","idempotencyKey":"answer-1","response":"default"}}"#;
    assert!(decode_request_frame(missing_revision).is_err());
    let open_answer = br#"{"jsonrpc":"2.0","id":"req_answer","method":"clarification.resolve","params":{"answers":[{"question":"Which database?","selectedOptions":[],"unexpected":true}],"clarificationId":"clarification_1","expectedRevision":"2","idempotencyKey":"answer-1","response":"default"}}"#;
    assert!(decode_request_frame(open_answer).is_err());
}

#[test]
fn generated_notifications_cover_host_event_and_subscription_close() {
    assert_eq!(
        Notification::parse("host.event"),
        Some(Notification::HostEvent)
    );
    assert_eq!(
        Notification::parse("subscription.closed"),
        Some(Notification::SubscriptionClosed)
    );
    assert_eq!(Notification::HostEvent.metadata().name, "host.event");
    assert_eq!(
        Notification::SubscriptionClosed.metadata().name,
        "subscription.closed"
    );

    let Ok(subscription_id) = SubscriptionId::new("sub_1") else {
        panic!("subscription ID fixture must be valid");
    };
    let notification = HostNotification {
        params: HostNotificationParams::SubscriptionClosed(Box::new(
            SubscriptionClosedNotificationParams {
                last_flushed_cursor: None,
                last_flushed_delivery_sequence: Some(DecimalU64::new(7)),
                reason: SubscriptionClosedReason::Terminal,
                subscription_id,
            },
        )),
    };
    let Ok(encoded) = encode_notification_frame(&notification) else {
        panic!("typed notification must encode");
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&encoded) else {
        panic!("encoded notification must be JSON");
    };
    assert_eq!(value["jsonrpc"], "2.0");
    assert_eq!(value["method"], "subscription.closed");
    assert_eq!(value["params"]["subscriptionId"], "sub_1");
    assert_eq!(value["params"]["lastFlushedDeliverySequence"], "7");
    assert!(value.get("id").is_none());

    let Ok(decoded) = decode_server_frame(&encoded, |_| None) else {
        panic!("strict client notification decoder must accept generated server output");
    };
    assert!(matches!(
        decoded,
        HostServerFrame::Notification(HostNotification {
            params: HostNotificationParams::SubscriptionClosed(_)
        })
    ));
    let mut open = value;
    open["unexpected"] = serde_json::json!(true);
    let Ok(open) = serde_json::to_vec(&open) else {
        panic!("open notification fixture must encode");
    };
    assert_eq!(
        decode_server_frame(&open, |_| None),
        Err(DecodeServerFrameError::InvalidEnvelope)
    );
}

#[test]
fn generated_event_profiles_are_the_exhaustive_eligibility_authority() {
    assert_eq!(
        EventProfile::ConversationV1.metadata().name,
        "conversation.v1"
    );
    assert!(EventProfile::ConversationV1.allows_event_class(EventClass::RunChanged));
    assert!(EventProfile::ConversationV1.allows_event_class(EventClass::ClarificationChanged));
    assert!(!EventProfile::ConversationV1.allows_event_class(EventClass::Diagnostic));
    assert_eq!(
        EventClass::parse("run_changed"),
        Some(EventClass::RunChanged)
    );
    assert_eq!(EventClass::parse("unknown"), None);
    assert_eq!(
        EventClass::RunChanged.metadata().schema_type,
        "RunChangedEvent"
    );
    assert_eq!(EventClass::RunChanged.metadata().feature, Some("runs"));
    assert_eq!(EventClass::RunChanged.metadata().scopes, &["run"]);
    assert!(EventClass::RunChanged.is_admitted(&["runs"], &["run"]));
    assert!(!EventClass::RunChanged.is_admitted(&[], &["run"]));
    assert!(!EventClass::RunChanged.is_admitted(&["runs"], &["read"]));

    assert_eq!(
        EventProfile::DesktopConversationV1.metadata().event_classes,
        EventProfile::ConversationV1.metadata().event_classes
    );
    assert!(EventProfile::OperationsV1.allows_event_class(EventClass::SessionChanged));
    assert!(EventProfile::OperationsV1.allows_event_class(EventClass::EnvironmentChanged));
    assert!(EventProfile::OperationsV1.allows_event_class(EventClass::Diagnostic));
    assert!(!EventProfile::OperationsV1.allows_event_class(EventClass::ApprovalChanged));
    assert!(
        EventProfile::ConversationV1
            .is_admitted(&["runs", "hitl", "clarifications"], &["run", "approval"])
    );
    assert!(
        !EventProfile::ConversationV1.is_admitted(&["runs", "hitl", "clarifications"], &["run"])
    );
}

#[test]
fn method_error_conversion_preserves_eligible_errors_and_sanitizes_ineligible_errors() {
    let invalid_params_data = InvalidParamsData {
        diagnostic_ref: Some("diag_safe".to_string()),
        kind: InvalidParamsDataKind::Value,
        reconciliation_required: false,
        resource_kind: Some("request".to_string()),
        retryable: false,
    };
    let converted = InitializeError::from(HostError {
        code: -32602,
        message: "invalid public params".to_string(),
        data: HostErrorData::InvalidParams(invalid_params_data.clone()),
    });
    assert!(matches!(
        converted,
        InitializeError::InvalidParams { message, data }
            if message == "invalid public params" && data == invalid_params_data
    ));

    let converted = InitializeError::from(HostError {
        code: -32010,
        message: "sensitive implementation detail".to_string(),
        data: HostErrorData::NotFound(NotFoundData {
            diagnostic_ref: Some("must-not-leak".to_string()),
            kind: NotFoundDataKind::Value,
            reconciliation_required: false,
            resource_kind: Some("private".to_string()),
            retryable: true,
        }),
    });
    assert!(matches!(
        converted,
        InitializeError::InternalError { message, data }
            if message == "internal error"
                && data.reconciliation_required
                && !data.retryable
                && data.diagnostic_ref.is_none()
                && data.resource_kind.is_none()
    ));
}

#[test]
fn decimal_u64_is_canonical_and_checked() {
    assert!(matches!(
        serde_json::from_str::<DecimalU64>(r#""0""#).map(DecimalU64::get),
        Ok(0)
    ));
    assert!(matches!(
        serde_json::from_str::<DecimalU64>(r#""18446744073709551615""#)
            .map(DecimalU64::get),
        Ok(value) if value == u64::MAX
    ));
    for invalid in [
        r#""00""#,
        r#""+1""#,
        r#"" 1""#,
        r#""18446744073709551616""#,
        "1",
    ] {
        assert!(serde_json::from_str::<DecimalU64>(invalid).is_err());
    }
    assert!(DecimalU64::new(u64::MAX).checked_increment().is_none());
}

#[test]
fn generated_method_parser_has_no_old_aliases() {
    assert_eq!(Method::parse("run.start"), Some(Method::RunStart));
    for removed in [
        "run.prompt",
        "run.await",
        "run.cancel",
        "stream.replay",
        "stream.subscribe",
        "storage.importLegacy",
    ] {
        assert_eq!(Method::parse(removed), None);
    }
}

#[test]
fn request_id_constructor_rejects_blank_values() {
    assert!(RequestId::new("request").is_ok());
    assert!(RequestId::new("   ").is_err());
}
