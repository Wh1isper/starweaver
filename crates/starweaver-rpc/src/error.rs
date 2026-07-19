//! RPC host internal error mapping.

use starweaver_rpc_core::{
    ALREADY_EXISTS, IDEMPOTENCY_CONFLICT, INVALID_PARAMS, NOT_FOUND, RUN_CONFLICT, RpcError,
    SERVER_ERROR, STALE_FENCE, STORAGE_UNAVAILABLE,
};
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
    /// Retryable durable storage failure.
    #[error("storage temporarily unavailable: {0}")]
    RetryableStorage(String),
    /// Agent runtime failure.
    #[error("runtime error: {0}")]
    Runtime(String),
    /// Requested durable or active resource was not found.
    #[error("not found: {0}")]
    NotFound(String),
    /// Requested durable record already exists.
    #[error("already exists: {0}")]
    AlreadyExists(String),
    /// Idempotency key was reused for a different command.
    #[error("idempotency conflict: {0}")]
    IdempotencyConflict(String),
    /// Durable lifecycle or active-run state conflicts with the request.
    #[error("run conflict: {0}")]
    RunConflict(String),
    /// Caller no longer owns the active fencing generation.
    #[error("stale fence: {0}")]
    StaleFence(String),
    /// Process I/O failure.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl From<starweaver_session::SessionStoreError> for RpcHostError {
    fn from(error: starweaver_session::SessionStoreError) -> Self {
        use starweaver_session::SessionStoreError;
        match error {
            SessionStoreError::NotFound(value) => Self::NotFound(value),
            SessionStoreError::AlreadyExists(value) => Self::AlreadyExists(value),
            SessionStoreError::IdempotencyConflict(value) => Self::IdempotencyConflict(value),
            SessionStoreError::Conflict(value)
            | SessionStoreError::QuotaExceeded(value)
            | SessionStoreError::RunConflict(value) => Self::RunConflict(value),
            SessionStoreError::StaleFence(value) => Self::StaleFence(value),
            SessionStoreError::RetryableStorage(value) => Self::RetryableStorage(value),
            SessionStoreError::Failed(value) => Self::Storage(value),
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
            RpcHostError::NotFound(message) => Self::new(NOT_FOUND, message),
            RpcHostError::AlreadyExists(message) => Self::new(ALREADY_EXISTS, message),
            RpcHostError::IdempotencyConflict(message) => Self::new(IDEMPOTENCY_CONFLICT, message),
            RpcHostError::RunConflict(message) => Self::new(RUN_CONFLICT, message),
            RpcHostError::StaleFence(message) => Self::new(STALE_FENCE, message),
            RpcHostError::RetryableStorage(message) => Self::new(STORAGE_UNAVAILABLE, message),
            RpcHostError::Storage(message) | RpcHostError::Runtime(message) => {
                Self::new(SERVER_ERROR, message)
            }
            RpcHostError::Io(error) => Self::new(SERVER_ERROR, error.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use starweaver_session::SessionStoreError;

    #[test]
    fn session_store_domain_errors_have_stable_rpc_codes() {
        let cases = [
            (SessionStoreError::NotFound("x".into()), NOT_FOUND),
            (SessionStoreError::AlreadyExists("x".into()), ALREADY_EXISTS),
            (
                SessionStoreError::IdempotencyConflict("x".into()),
                IDEMPOTENCY_CONFLICT,
            ),
            (SessionStoreError::RunConflict("x".into()), RUN_CONFLICT),
            (SessionStoreError::Conflict("x".into()), RUN_CONFLICT),
            (SessionStoreError::QuotaExceeded("x".into()), RUN_CONFLICT),
            (SessionStoreError::StaleFence("x".into()), STALE_FENCE),
            (
                SessionStoreError::RetryableStorage("x".into()),
                STORAGE_UNAVAILABLE,
            ),
            (SessionStoreError::Failed("x".into()), SERVER_ERROR),
        ];
        for (error, expected) in cases {
            let rpc = RpcError::from(RpcHostError::from(error));
            assert_eq!(rpc.code, expected);
        }
    }
}
