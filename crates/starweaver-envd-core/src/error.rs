//! `EnvD` error contracts.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Result alias for envd operations.
pub type EnvdResult<T> = Result<T, EnvdError>;

/// Error category for envd operations.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvdErrorCode {
    /// Input was not valid for the selected environment.
    InvalidRequest,
    /// Access was denied by environment policy.
    AccessDenied,
    /// Requested resource was not found.
    NotFound,
    /// The operation is not supported by the selected environment.
    Unsupported,
    /// The provider failed while executing the request.
    Provider,
}

/// Error returned by envd service operations.
#[derive(Clone, Debug, Deserialize, Error, Eq, PartialEq, Serialize)]
#[error("{code:?}: {message}")]
pub struct EnvdError {
    /// Machine-readable error category.
    pub code: EnvdErrorCode,
    /// Human-readable error message.
    pub message: String,
}

impl EnvdError {
    /// Create an envd error.
    #[must_use]
    pub fn new(code: EnvdErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// Create an invalid request error.
    #[must_use]
    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::new(EnvdErrorCode::InvalidRequest, message)
    }

    /// Create an access denied error.
    #[must_use]
    pub fn access_denied(message: impl Into<String>) -> Self {
        Self::new(EnvdErrorCode::AccessDenied, message)
    }

    /// Create a not found error.
    #[must_use]
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(EnvdErrorCode::NotFound, message)
    }

    /// Create an unsupported operation error.
    #[must_use]
    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new(EnvdErrorCode::Unsupported, message)
    }

    /// Create a provider error.
    #[must_use]
    pub fn provider(message: impl Into<String>) -> Self {
        Self::new(EnvdErrorCode::Provider, message)
    }
}
