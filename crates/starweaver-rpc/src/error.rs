//! RPC host internal error mapping.

use starweaver_rpc_core::{INVALID_PARAMS, RpcError, SERVER_ERROR};
use thiserror::Error;

/// Result returned by standalone RPC host internals.
pub type RpcHostResult<T> = Result<T, RpcHostError>;

/// Errors owned by the standalone RPC product.
#[derive(Debug, Error)]
pub enum RpcHostError {
    /// Invalid product input or configuration.
    #[error("{0}")]
    Invalid(String),
    /// Durable storage failure.
    #[error("storage error: {0}")]
    Storage(String),
    /// Agent runtime failure.
    #[error("runtime error: {0}")]
    Runtime(String),
    /// Requested durable or active resource was not found.
    #[error("not found: {0}")]
    NotFound(String),
    /// Process I/O failure.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl From<starweaver_session::SessionStoreError> for RpcHostError {
    fn from(error: starweaver_session::SessionStoreError) -> Self {
        match error {
            starweaver_session::SessionStoreError::NotFound(value) => Self::NotFound(value),
            other => Self::Storage(other.to_string()),
        }
    }
}

impl From<starweaver_stream::ReplayError> for RpcHostError {
    fn from(error: starweaver_stream::ReplayError) -> Self {
        Self::Storage(error.to_string())
    }
}

impl From<RpcHostError> for RpcError {
    fn from(error: RpcHostError) -> Self {
        match error {
            RpcHostError::Invalid(message) => Self::new(INVALID_PARAMS, message),
            other => Self::new(SERVER_ERROR, other.to_string()),
        }
    }
}
