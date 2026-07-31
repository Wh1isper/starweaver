#![allow(clippy::expect_used)]

//! Deterministic Computer Use service and router conformance tests.

use std::{sync::Arc, time::Duration};

use starweaver_computer_use::{
    AffineTransform2D, COMPUTER_CLICK_TOOL, COMPUTER_OBSERVE_TOOL, ClickAction, ComputerAction,
    ComputerActionRequest, ComputerCapabilityGrant, ComputerSessionState, ComputerToolContent,
    ComputerToolGrant, ComputerToolInvocation, ComputerToolRouter, ComputerUseError,
    ComputerUseErrorCode, ComputerUseFailure, ComputerUsePolicy, ComputerUseService, EffectStatus,
    FakeComputerUseConfig, FakeComputerUseService, InvocationId, InvocationSource, ModelPoint,
    ObservationRef, ObserveRequest, OperationId, PermissionCapabilityStatus, PermissionRequest,
    PointerButton, RetryClassification,
};
use starweaver_core::CancellationToken;

fn full_policy() -> ComputerUsePolicy {
    ComputerUsePolicy {
        allowed_capabilities: ComputerCapabilityGrant {
            observe: true,
            pointer: true,
            keyboard: true,
            accessibility_snapshot: false,
        },
        post_action_settle: Duration::ZERO,
        ..ComputerUsePolicy::default()
    }
}

async fn observed_session() -> (
    FakeComputerUseService,
    starweaver_computer_use::DynComputerSession,
    starweaver_computer_use::ComputerObservation,
) {
    let service = FakeComputerUseService::new(full_policy(), FakeComputerUseConfig::default());
    let session = service
        .open_current_desktop(CancellationToken::new())
        .await
        .expect("fake session should open");
    let observation = session
        .observe(
            ObserveRequest {
                operation_id: OperationId::new(),
                include_accessibility: false,
            },
            CancellationToken::new(),
        )
        .await
        .expect("fake observation should succeed");
    (service, session, observation)
}

const fn click_request(
    operation_id: OperationId,
    observation_id: starweaver_computer_use::ObservationId,
    x: u32,
) -> ComputerActionRequest {
    ComputerActionRequest {
        operation_id,
        observation: ObservationRef { observation_id },
        action: ComputerAction::Click(ClickAction {
            point: ModelPoint { x, y: 20 },
            button: PointerButton::Left,
            click_count: 1,
            modifiers: Vec::new(),
        }),
    }
}

#[tokio::test]
async fn requested_accessibility_snapshot_is_bounded_and_structured() {
    let mut policy = full_policy();
    policy.allowed_capabilities.accessibility_snapshot = true;
    let mut config = FakeComputerUseConfig::default();
    config.capabilities.accessibility_snapshot = true;
    let service = FakeComputerUseService::new(policy, config);

    let permission = service
        .request_permissions(
            PermissionRequest {
                screen_recording: true,
                accessibility: true,
            },
            CancellationToken::new(),
        )
        .await
        .expect("fake permission request should return immediate status");
    assert_eq!(
        permission.permissions.accessibility,
        PermissionCapabilityStatus::Granted
    );
    assert!(permission.effective_capabilities.accessibility_snapshot);

    let session = service
        .open_current_desktop(CancellationToken::new())
        .await
        .expect("fake session should open");
    let observation = session
        .observe(
            ObserveRequest {
                operation_id: OperationId::new(),
                include_accessibility: true,
            },
            CancellationToken::new(),
        )
        .await
        .expect("accessibility observation should succeed");
    let snapshot = observation
        .accessibility
        .expect("requested accessibility snapshot must be present");
    assert_eq!(snapshot.generation.0, 1);
    assert!(!snapshot.truncated);
    assert_eq!(snapshot.nodes.len(), 1);
    assert_eq!(snapshot.nodes[0].role, "AXApplication");
    assert_eq!(snapshot.nodes[0].state.protected, Some(false));
    assert_eq!(
        snapshot.nodes[0]
            .model_bounds
            .expect("fake root bounds should be present")
            .width,
        320
    );
}

#[tokio::test]
async fn service_rejects_accessibility_snapshot_over_policy_budget() {
    let mut policy = full_policy();
    policy.allowed_capabilities.accessibility_snapshot = true;
    policy.accessibility.max_nodes = 0;
    let mut config = FakeComputerUseConfig::default();
    config.capabilities.accessibility_snapshot = true;
    let service = FakeComputerUseService::new(policy, config);
    let session = service
        .open_current_desktop(CancellationToken::new())
        .await
        .expect("fake session should open");

    let error = session
        .observe(
            ObserveRequest {
                operation_id: OperationId::new(),
                include_accessibility: true,
            },
            CancellationToken::new(),
        )
        .await
        .expect_err("service must reject backend data over the host budget");
    assert_eq!(error.code, ComputerUseErrorCode::BackendUnavailable);
}

#[tokio::test]
async fn initial_accessibility_prompt_does_not_invalidate_pixel_session() {
    let mut policy = full_policy();
    policy.allowed_capabilities.accessibility_snapshot = true;
    let service = FakeComputerUseService::new(policy, FakeComputerUseConfig::default());
    let session = service
        .open_current_desktop(CancellationToken::new())
        .await
        .expect("fake session should open");
    service
        .backend()
        .fail_next_observe(ComputerUseError::new(
            ComputerUseErrorCode::PermissionRequired,
            "injected initial accessibility permission request",
            RetryClassification::AfterPermissionChange,
        ))
        .await;

    let error = session
        .observe(
            ObserveRequest {
                operation_id: OperationId::new(),
                include_accessibility: true,
            },
            CancellationToken::new(),
        )
        .await
        .expect_err("initial permission request should remain authoritative");
    assert_eq!(error.code, ComputerUseErrorCode::PermissionRequired);
    assert_eq!(
        session
            .status(CancellationToken::new())
            .await
            .expect("pixel session should remain probeable")
            .state,
        ComputerSessionState::ReadyControl
    );
}

#[tokio::test]
async fn accessibility_revocation_preserves_proven_pixel_authority() {
    let mut policy = full_policy();
    policy.allowed_capabilities.accessibility_snapshot = true;
    let mut config = FakeComputerUseConfig::default();
    config.capabilities.accessibility_snapshot = true;
    let service = FakeComputerUseService::new(policy, config);
    let session = service
        .open_current_desktop(CancellationToken::new())
        .await
        .expect("fake session should open");
    assert!(session.capabilities().accessibility_snapshot);

    service
        .backend()
        .set_capabilities(starweaver_computer_use::EffectiveComputerCapabilities {
            observe: true,
            pointer: true,
            keyboard: true,
            accessibility_snapshot: false,
        })
        .await;
    service
        .backend()
        .fail_next_observe(ComputerUseError::new(
            ComputerUseErrorCode::PermissionRequired,
            "injected Accessibility revocation",
            RetryClassification::AfterPermissionChange,
        ))
        .await;

    let error = session
        .observe(
            ObserveRequest {
                operation_id: OperationId::new(),
                include_accessibility: true,
            },
            CancellationToken::new(),
        )
        .await
        .expect_err("revoked Accessibility authority must reject semantic capture");
    assert_eq!(error.code, ComputerUseErrorCode::PermissionRequired);
    assert!(session.capabilities().observe);
    assert!(!session.capabilities().accessibility_snapshot);
    assert_eq!(
        session
            .status(CancellationToken::new())
            .await
            .expect("pixel session should remain available")
            .state,
        ComputerSessionState::ReadyControl
    );
}

#[tokio::test]
async fn generic_permission_error_invalidates_session_when_pixel_authority_was_revoked() {
    let mut policy = full_policy();
    policy.allowed_capabilities.accessibility_snapshot = true;
    let service = FakeComputerUseService::new(policy, FakeComputerUseConfig::default());
    let session = service
        .open_current_desktop(CancellationToken::new())
        .await
        .expect("fake session should open");
    service
        .backend()
        .set_capabilities(starweaver_computer_use::EffectiveComputerCapabilities {
            observe: false,
            pointer: false,
            keyboard: false,
            accessibility_snapshot: false,
        })
        .await;
    service
        .backend()
        .fail_next_observe(ComputerUseError::new(
            ComputerUseErrorCode::PermissionRequired,
            "injected ambiguous permission loss",
            RetryClassification::AfterPermissionChange,
        ))
        .await;

    let error = session
        .observe(
            ObserveRequest {
                operation_id: OperationId::new(),
                include_accessibility: true,
            },
            CancellationToken::new(),
        )
        .await
        .expect_err("revoked pixel authority must invalidate the session");
    assert_eq!(error.code, ComputerUseErrorCode::PermissionRequired);
    assert!(!session.capabilities().observe);
    assert_eq!(
        session
            .status(CancellationToken::new())
            .await
            .expect("invalidated session should remain inspectable")
            .state,
        ComputerSessionState::SessionUnavailable
    );
}

#[tokio::test]
async fn ordinary_observation_refreshes_revoked_accessibility_capability() {
    let mut policy = full_policy();
    policy.allowed_capabilities.accessibility_snapshot = true;
    let mut config = FakeComputerUseConfig::default();
    config.capabilities.accessibility_snapshot = true;
    let service = FakeComputerUseService::new(policy, config);
    let session = service
        .open_current_desktop(CancellationToken::new())
        .await
        .expect("fake session should open");
    assert!(session.capabilities().accessibility_snapshot);

    service
        .backend()
        .set_capabilities(starweaver_computer_use::EffectiveComputerCapabilities {
            observe: true,
            pointer: true,
            keyboard: true,
            accessibility_snapshot: false,
        })
        .await;
    let observation = session
        .observe(
            ObserveRequest {
                operation_id: OperationId::new(),
                include_accessibility: false,
            },
            CancellationToken::new(),
        )
        .await
        .expect("pixel-only observation should remain available");

    assert!(!observation.capabilities.accessibility_snapshot);
    assert!(!session.capabilities().accessibility_snapshot);
}

#[tokio::test]
async fn accessibility_request_cannot_widen_host_policy() {
    let service = FakeComputerUseService::new(full_policy(), FakeComputerUseConfig::default());
    let session = service
        .open_current_desktop(CancellationToken::new())
        .await
        .expect("fake session should open");
    let error = session
        .observe(
            ObserveRequest {
                operation_id: OperationId::new(),
                include_accessibility: true,
            },
            CancellationToken::new(),
        )
        .await
        .expect_err("accessibility must remain denied by host policy");
    assert_eq!(error.code, ComputerUseErrorCode::PolicyDenied);
}

#[tokio::test]
async fn action_advances_effect_epoch_and_returns_fresh_observation() {
    let (service, session, observation) = observed_session().await;
    let result = session
        .act(
            click_request(OperationId::new(), observation.observation_id, 10),
            CancellationToken::new(),
        )
        .await
        .expect("fake click should execute");

    assert_eq!(result.receipt.effect_status, EffectStatus::Executed);
    assert_eq!(result.receipt.basis_effect_epoch.0, 0);
    assert_eq!(result.receipt.resulting_effect_epoch.0, 1);
    assert_eq!(result.observation.effect_epoch.0, 1);
    assert_ne!(
        result.receipt.basis_observation_id,
        result.observation.observation_id
    );
    assert_eq!(service.backend().recorded_actions().await.len(), 1);
}

#[tokio::test]
async fn another_effect_invalidates_every_older_observation() {
    let (_service, session, first) = observed_session().await;
    let second = session
        .observe(
            ObserveRequest {
                operation_id: OperationId::new(),
                include_accessibility: false,
            },
            CancellationToken::new(),
        )
        .await
        .expect("second observation should succeed");
    session
        .act(
            click_request(OperationId::new(), second.observation_id, 10),
            CancellationToken::new(),
        )
        .await
        .expect("first effect should execute");

    let failure = session
        .act(
            click_request(OperationId::new(), first.observation_id, 11),
            CancellationToken::new(),
        )
        .await
        .expect_err("old observation must be stale");
    assert_eq!(failure.effect_status, EffectStatus::NotExecuted);
    assert_eq!(failure.error.code, ComputerUseErrorCode::StaleObservation);
    assert_eq!(
        failure.error.retry,
        RetryClassification::AfterFreshObservation
    );
}

#[tokio::test]
async fn exact_duplicate_never_reexecutes_and_conflict_fails_closed() {
    let (service, session, observation) = observed_session().await;
    let operation_id = OperationId::new();
    let request = click_request(operation_id.clone(), observation.observation_id.clone(), 10);
    session
        .act(request.clone(), CancellationToken::new())
        .await
        .expect("first effect should execute");

    let duplicate = session
        .act(request, CancellationToken::new())
        .await
        .expect_err("completed duplicate should not reexecute");
    assert_eq!(
        duplicate.error.code,
        ComputerUseErrorCode::DuplicateResultEvicted
    );
    assert_eq!(duplicate.effect_status, EffectStatus::Executed);
    assert!(duplicate.receipt.is_some());

    let conflict = session
        .act(
            click_request(operation_id, observation.observation_id, 12),
            CancellationToken::new(),
        )
        .await
        .expect_err("mismatched duplicate should fail");
    assert_eq!(
        conflict.error.code,
        ComputerUseErrorCode::IdempotencyConflict
    );
    assert_eq!(conflict.effect_status, EffectStatus::NotExecuted);
    assert_eq!(service.backend().recorded_actions().await.len(), 1);
}

#[tokio::test]
async fn out_of_range_coordinates_are_rejected_not_clamped() {
    let (service, session, observation) = observed_session().await;
    let failure = session
        .act(
            click_request(
                OperationId::new(),
                observation.observation_id,
                observation.geometry.model_size_px.width,
            ),
            CancellationToken::new(),
        )
        .await
        .expect_err("boundary coordinate must be rejected");
    assert_eq!(failure.error.code, ComputerUseErrorCode::InvalidCoordinate);
    assert_eq!(failure.effect_status, EffectStatus::NotExecuted);
    assert!(service.backend().recorded_actions().await.is_empty());
}

#[tokio::test]
async fn post_action_capture_failure_preserves_executed_receipt() {
    let (service, session, observation) = observed_session().await;
    service
        .backend()
        .fail_next_observe(starweaver_computer_use::ComputerUseError::new(
            ComputerUseErrorCode::CaptureInterrupted,
            "injected capture failure",
            RetryClassification::AfterFreshObservation,
        ))
        .await;
    let failure = session
        .act(
            click_request(OperationId::new(), observation.observation_id, 10),
            CancellationToken::new(),
        )
        .await
        .expect_err("post-action capture failure should be evidence-bearing");
    assert_eq!(failure.effect_status, EffectStatus::Executed);
    assert_eq!(failure.error.code, ComputerUseErrorCode::CaptureInterrupted);
    assert!(failure.receipt.is_some());
    assert_eq!(service.backend().recorded_actions().await.len(), 1);
}

#[tokio::test]
async fn router_rejects_unknown_fields_and_maps_exact_image() {
    let service = Arc::new(FakeComputerUseService::new(
        full_policy(),
        FakeComputerUseConfig::default(),
    ));
    let router = ComputerToolRouter::new(
        service,
        starweaver_computer_use::ComputerSessionBinding::ServiceOwnedLazy,
        ComputerToolGrant {
            observe: true,
            pointer: true,
            keyboard: false,
        },
    );
    let invalid = router
        .call(
            ComputerToolInvocation::new(InvocationId::new(), InvocationSource::DirectTest),
            COMPUTER_OBSERVE_TOOL,
            serde_json::json!({"unexpected": true}),
            CancellationToken::new(),
        )
        .await;
    assert!(invalid.is_error);
    assert_eq!(
        invalid
            .structured
            .error
            .as_ref()
            .map(|error| error.code.as_str()),
        Some("invalid_request")
    );

    let observed = router
        .call(
            ComputerToolInvocation::new(InvocationId::new(), InvocationSource::DirectTest),
            COMPUTER_OBSERVE_TOOL,
            serde_json::json!({}),
            CancellationToken::new(),
        )
        .await;
    assert!(!observed.is_error);
    let view = observed
        .structured
        .observation
        .as_ref()
        .expect("structured observation should exist");
    let Some(ComputerToolContent::Image {
        width,
        height,
        sha256,
        observation_id,
        ..
    }) = observed.content.first()
    else {
        panic!("exactly one image should be returned");
    };
    assert_eq!((*width, *height), (view.image.width, view.image.height));
    assert_eq!(sha256, &view.image.sha256);
    assert_eq!(observation_id, &view.observation_id);

    let click = router
        .call(
            ComputerToolInvocation::new(InvocationId::new(), InvocationSource::DirectTest),
            COMPUTER_CLICK_TOOL,
            serde_json::json!({
                "observation_id": view.observation_id,
                "x": 10,
                "y": 20
            }),
            CancellationToken::new(),
        )
        .await;
    assert!(!click.is_error);
    assert_eq!(click.content.len(), 1);
    assert_eq!(
        click
            .structured
            .receipt
            .as_ref()
            .map(|receipt| receipt.effect_status),
        Some(EffectStatus::Executed)
    );
}

#[test]
fn affine_transform_round_trip_supports_negative_native_origins() {
    let transform = AffineTransform2D::checked([2.0, 0.0, -400.0, 0.0, 1.5, -90.0, 0.0, 0.0, 1.0])
        .expect("transform should be valid");
    let inverse = transform.inverse().expect("inverse should exist");
    let point = ModelPoint { x: 123, y: 45 };
    let native = transform.apply(point);
    let model_x =
        inverse.values[0].mul_add(native.x, inverse.values[1] * native.y) + inverse.values[2];
    let model_y =
        inverse.values[3].mul_add(native.x, inverse.values[4] * native.y) + inverse.values[5];
    assert!((model_x - f64::from(point.x)).abs() < 1.0e-8);
    assert!((model_y - f64::from(point.y)).abs() < 1.0e-8);
}

#[tokio::test]
async fn cancellation_before_queue_admission_is_not_executed() {
    let (_service, session, observation) = observed_session().await;
    let cancel = CancellationToken::new();
    cancel.cancel();
    let result = session
        .act(
            click_request(OperationId::new(), observation.observation_id, 10),
            cancel,
        )
        .await;
    assert!(matches!(
        result,
        Err(ComputerUseFailure {
            effect_status: EffectStatus::NotExecuted,
            ..
        })
    ));
}

#[tokio::test]
async fn status_intersects_backend_capabilities_with_policy() {
    let mut policy = full_policy();
    policy.allowed_capabilities = ComputerCapabilityGrant::default();
    let service = FakeComputerUseService::new(policy, FakeComputerUseConfig::default());
    let status = service
        .status(CancellationToken::new())
        .await
        .expect("status probe should succeed");

    assert!(!status.effective_capabilities.observe);
    assert!(!status.effective_capabilities.pointer);
    assert!(!status.effective_capabilities.keyboard);
}

#[tokio::test]
async fn observation_ledger_evicts_oldest_record_at_capacity() {
    let mut policy = full_policy();
    policy.max_observations = 2;
    let service = FakeComputerUseService::new(policy, FakeComputerUseConfig::default());
    let session = service
        .open_current_desktop(CancellationToken::new())
        .await
        .expect("fake session should open");
    let mut observations = Vec::new();
    for _ in 0..3 {
        observations.push(
            session
                .observe(
                    ObserveRequest {
                        operation_id: OperationId::new(),
                        include_accessibility: false,
                    },
                    CancellationToken::new(),
                )
                .await
                .expect("fake observation should succeed"),
        );
    }

    let first = observations.remove(0);
    let failure = session
        .act(
            click_request(OperationId::new(), first.observation_id, 10),
            CancellationToken::new(),
        )
        .await
        .expect_err("oldest observation should be evicted");
    assert_eq!(failure.error.code, ComputerUseErrorCode::StaleObservation);
}

#[tokio::test]
async fn status_reports_latest_layout_generation_deterministically() {
    let service = FakeComputerUseService::new(full_policy(), FakeComputerUseConfig::default());
    let session = service
        .open_current_desktop(CancellationToken::new())
        .await
        .expect("fake session should open");
    session
        .observe(
            ObserveRequest {
                operation_id: OperationId::new(),
                include_accessibility: false,
            },
            CancellationToken::new(),
        )
        .await
        .expect("initial observation should succeed");
    service
        .backend()
        .change_layout(starweaver_computer_use::PixelSize {
            width: 400,
            height: 240,
        })
        .await;
    let latest = session
        .observe(
            ObserveRequest {
                operation_id: OperationId::new(),
                include_accessibility: false,
            },
            CancellationToken::new(),
        )
        .await
        .expect("observation after layout change should succeed");
    let status = session
        .status(CancellationToken::new())
        .await
        .expect("session status should succeed");

    assert_eq!(status.layout_generation, Some(latest.layout_generation));
}
