//! OAuth error types.

use std::path::PathBuf;

use serde_json::Value;
use thiserror::Error;

/// OAuth result alias.
pub type OAuthResult<T> = Result<T, OAuthError>;

/// OAuth storage and provider error.
#[derive(Debug, Error)]
pub enum OAuthError {
    /// Filesystem operation failed.
    #[error("filesystem error at {}: {source}", path.display())]
    Io {
        /// Path involved in the failure.
        path: PathBuf,
        /// Source IO error.
        #[source]
        source: std::io::Error,
    },
    /// JSON serialization failed.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    /// HTTP transport failed.
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    /// Provider returned a non-success status.
    #[error("provider status {status}: {body}")]
    ProviderStatus {
        /// HTTP status code.
        status: u16,
        /// Provider response body.
        body: Value,
    },
    /// Provider record is missing from the auth store.
    #[error("OAuth provider is not logged in: {provider}")]
    NotLoggedIn {
        /// Provider name.
        provider: String,
    },
    /// Refresh token is missing from the provider record.
    #[error("OAuth provider {provider} is missing a refresh token")]
    MissingRefreshToken {
        /// Provider name.
        provider: String,
    },
    /// OAuth response did not include a required field.
    #[error("invalid OAuth response: {0}")]
    InvalidResponse(String),
    /// JWT payload decoding failed.
    #[error("invalid JWT payload: {0}")]
    InvalidJwt(String),
    /// Refresh returned a token for a different account.
    #[error("Codex refresh returned a different account; log in again")]
    AccountMismatch,
    /// Device authorization timed out.
    #[error("Codex device authorization timed out")]
    DeviceAuthorizationTimeout,
}

pub fn io_error(path: impl Into<PathBuf>, source: std::io::Error) -> OAuthError {
    OAuthError::Io {
        path: path.into(),
        source,
    }
}
