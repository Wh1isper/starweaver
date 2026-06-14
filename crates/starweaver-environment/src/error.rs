//! Environment provider errors and result aliases.

use thiserror::Error;

/// Environment operation failure.
#[derive(Debug, Error)]
pub enum EnvironmentError {
    /// Access was denied by policy.
    #[error("environment access denied: {0}")]
    AccessDenied(String),
    /// Requested resource was not found.
    #[error("environment resource not found: {0}")]
    NotFound(String),
    /// Input was invalid for this provider.
    #[error("invalid environment request: {0}")]
    InvalidRequest(String),
    /// Provider execution failed.
    #[error("environment provider failed: {0}")]
    Provider(String),
}

/// Result alias for environment provider operations.
pub type EnvironmentResult<T> = Result<T, EnvironmentError>;
