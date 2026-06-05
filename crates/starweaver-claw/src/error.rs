//! Error types for Starweaver Claw.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;
use starweaver_session::SessionStoreError;
use starweaver_stream::ReplayError;
use thiserror::Error;

/// Result alias for Claw operations.
pub type ClawResult<T> = Result<T, ClawError>;

/// Service-level error.
#[derive(Debug, Error)]
pub enum ClawError {
    /// Requested resource was missing.
    #[error("{0}")]
    NotFound(String),
    /// Request failed validation.
    #[error("{0}")]
    InvalidRequest(String),
    /// Request conflicts with current resource state.
    #[error("{0}")]
    Conflict(String),
    /// Caller lacks API authorization.
    #[error("{0}")]
    Unauthorized(String),
    /// Session store failed.
    #[error(transparent)]
    SessionStore(#[from] SessionStoreError),
    /// Replay/event store failed.
    #[error(transparent)]
    Replay(#[from] ReplayError),
    /// I/O failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// Serialization failed.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// YAML serialization failed.
    #[error(transparent)]
    Yaml(#[from] serde_yaml::Error),
    /// Runtime failed.
    #[error("{0}")]
    Failed(String),
}

impl ClawError {
    /// Return an HTTP status code for this error.
    #[must_use]
    pub const fn status_code(&self) -> StatusCode {
        match self {
            Self::NotFound(_) | Self::SessionStore(SessionStoreError::NotFound(_)) => {
                StatusCode::NOT_FOUND
            }
            Self::InvalidRequest(_) => StatusCode::UNPROCESSABLE_ENTITY,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            Self::SessionStore(_)
            | Self::Replay(_)
            | Self::Io(_)
            | Self::Json(_)
            | Self::Yaml(_)
            | Self::Failed(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: ErrorPayload,
}

#[derive(Debug, Serialize)]
struct ErrorPayload {
    code: String,
    message: String,
}

impl IntoResponse for ClawError {
    fn into_response(self) -> Response {
        let code = match &self {
            Self::NotFound(_) | Self::SessionStore(SessionStoreError::NotFound(_)) => "not_found",
            Self::InvalidRequest(_) => "invalid_request",
            Self::Conflict(_) => "conflict",
            Self::Unauthorized(_) => "unauthorized",
            Self::SessionStore(_) => "session_store_error",
            Self::Replay(_) => "replay_error",
            Self::Io(_) => "io_error",
            Self::Json(_) => "json_error",
            Self::Yaml(_) => "yaml_error",
            Self::Failed(_) => "failed",
        };
        let status = self.status_code();
        let body = ErrorBody {
            error: ErrorPayload {
                code: code.to_string(),
                message: self.to_string(),
            },
        };
        (status, Json(body)).into_response()
    }
}
