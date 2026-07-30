use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{ComputerActionReceipt, EffectStatus};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ComputerUseErrorCode {
    InvalidRequest,
    PolicyDenied,
    UnsupportedPlatform,
    UnsupportedCapability,
    PermissionRequired,
    PermissionDenied,
    PermissionRestartRequired,
    SessionInactive,
    SessionLocked,
    SessionChanged,
    SecureDesktopUnavailable,
    UserPresenceRequired,
    UserPresenceRevoked,
    SessionAlreadyOpen,
    SessionClosed,
    StaleProcess,
    StaleSession,
    StaleTarget,
    StaleLayout,
    StaleObservation,
    ObservationExpired,
    InvalidCoordinate,
    InvalidTransform,
    DisplayTopologyChanged,
    ProtectedOrRedactedFrame,
    CaptureInterrupted,
    ImageLimitExceeded,
    UnsupportedKey,
    UnsupportedText,
    InputRejected,
    InputDeliveryUncertain,
    InputCleanupFailed,
    IdempotencyConflict,
    DuplicateResultEvicted,
    Cancelled,
    TimedOut,
    BackendUnavailable,
    Internal,
}

impl ComputerUseErrorCode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::PolicyDenied => "policy_denied",
            Self::UnsupportedPlatform => "unsupported_platform",
            Self::UnsupportedCapability => "unsupported_capability",
            Self::PermissionRequired => "permission_required",
            Self::PermissionDenied => "permission_denied",
            Self::PermissionRestartRequired => "permission_restart_required",
            Self::SessionInactive => "session_inactive",
            Self::SessionLocked => "session_locked",
            Self::SessionChanged => "session_changed",
            Self::SecureDesktopUnavailable => "secure_desktop_unavailable",
            Self::UserPresenceRequired => "user_presence_required",
            Self::UserPresenceRevoked => "user_presence_revoked",
            Self::SessionAlreadyOpen => "session_already_open",
            Self::SessionClosed => "session_closed",
            Self::StaleProcess => "stale_process",
            Self::StaleSession => "stale_session",
            Self::StaleTarget => "stale_target",
            Self::StaleLayout => "stale_layout",
            Self::StaleObservation => "stale_observation",
            Self::ObservationExpired => "observation_expired",
            Self::InvalidCoordinate => "invalid_coordinate",
            Self::InvalidTransform => "invalid_transform",
            Self::DisplayTopologyChanged => "display_topology_changed",
            Self::ProtectedOrRedactedFrame => "protected_or_redacted_frame",
            Self::CaptureInterrupted => "capture_interrupted",
            Self::ImageLimitExceeded => "image_limit_exceeded",
            Self::UnsupportedKey => "unsupported_key",
            Self::UnsupportedText => "unsupported_text",
            Self::InputRejected => "input_rejected",
            Self::InputDeliveryUncertain => "input_delivery_uncertain",
            Self::InputCleanupFailed => "input_cleanup_failed",
            Self::IdempotencyConflict => "idempotency_conflict",
            Self::DuplicateResultEvicted => "duplicate_result_evicted",
            Self::Cancelled => "cancelled",
            Self::TimedOut => "timed_out",
            Self::BackendUnavailable => "backend_unavailable",
            Self::Internal => "internal",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RetryClassification {
    Never,
    AfterFreshObservation,
    AfterPermissionChange,
    AfterExplicitResume,
    NewSessionRequired,
    EffectStatusDependent,
}

#[derive(Clone, Debug, Error, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[error("{code:?}: {message}")]
pub struct ComputerUseError {
    pub code: ComputerUseErrorCode,
    pub message: String,
    pub retry: RetryClassification,
    #[serde(default)]
    pub remediation: Vec<String>,
    pub diagnostics_id: Option<String>,
}

impl ComputerUseError {
    #[must_use]
    pub fn new(
        code: ComputerUseErrorCode,
        message: impl Into<String>,
        retry: RetryClassification,
    ) -> Self {
        let mut message = message.into();
        message.truncate(512);
        Self {
            code,
            message,
            retry,
            remediation: Vec::new(),
            diagnostics_id: None,
        }
    }

    #[must_use]
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(
            ComputerUseErrorCode::InvalidRequest,
            message,
            RetryClassification::Never,
        )
    }

    #[must_use]
    pub fn cancelled() -> Self {
        Self::new(
            ComputerUseErrorCode::Cancelled,
            "computer operation was cancelled",
            RetryClassification::Never,
        )
    }

    #[must_use]
    pub fn unsupported_platform() -> Self {
        Self::new(
            ComputerUseErrorCode::UnsupportedPlatform,
            "computer use is unavailable on this platform",
            RetryClassification::Never,
        )
    }
}

#[derive(Clone, Debug, Error, PartialEq)]
#[error("computer action failed with {effect_status:?}: {error}")]
pub struct ComputerUseFailure {
    pub error: ComputerUseError,
    pub effect_status: EffectStatus,
    pub receipt: Option<ComputerActionReceipt>,
}

impl ComputerUseFailure {
    #[must_use]
    pub const fn not_executed(error: ComputerUseError) -> Self {
        Self {
            error,
            effect_status: EffectStatus::NotExecuted,
            receipt: None,
        }
    }
}
