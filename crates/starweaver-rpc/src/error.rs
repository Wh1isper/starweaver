//! RPC host internal error mapping.

use starweaver_rpc_core::generated as host;
use thiserror::Error;

pub(crate) const ALREADY_EXISTS: i64 = host::ERROR_CODE_ALREADY_EXISTS;
pub(crate) const ENVIRONMENT_UNAVAILABLE: i64 = host::ERROR_CODE_ENVIRONMENT_UNAVAILABLE;
pub(crate) const IDEMPOTENCY_CONFLICT: i64 = host::ERROR_CODE_IDEMPOTENCY_CONFLICT;
pub(crate) const INVALID_PARAMS: i64 = host::ERROR_CODE_INVALID_PARAMS;
pub(crate) const NOT_FOUND: i64 = host::ERROR_CODE_NOT_FOUND;
pub(crate) const RUN_CONFLICT: i64 = host::ERROR_CODE_RUN_CONFLICT;
pub(crate) const SERVER_ERROR: i64 = host::ERROR_CODE_INTERNAL_ERROR;
pub(crate) const SESSION_SEARCH_UNAVAILABLE: i64 = host::ERROR_CODE_SESSION_SEARCH_UNAVAILABLE;
pub(crate) const STALE_FENCE: i64 = host::ERROR_CODE_STALE_FENCE;
pub(crate) const STORAGE_UNAVAILABLE: i64 = host::ERROR_CODE_STORAGE_UNAVAILABLE;
pub(crate) const UNSUPPORTED_FEATURE: i64 = host::ERROR_CODE_UNSUPPORTED_FEATURE;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RpcError {
    pub(crate) code: i64,
    pub(crate) message: String,
}

impl RpcError {
    pub(crate) fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

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
            RpcHostError::RetryableStorage(_) => Self::new(
                STORAGE_UNAVAILABLE,
                "durable storage is temporarily unavailable",
            ),
            RpcHostError::Storage(_) => Self::new(SERVER_ERROR, "durable storage operation failed"),
            RpcHostError::Runtime(_) => Self::new(SERVER_ERROR, "runtime operation failed"),
            RpcHostError::Io(_) => Self::new(SERVER_ERROR, "host I/O operation failed"),
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

    #[test]
    fn internal_errors_are_redacted_from_rpc_messages() {
        let secret = "sqlite:///private/path?token=provider-secret";
        let cases = [
            RpcHostError::from(SessionStoreError::Failed(secret.to_string())),
            RpcHostError::from(SessionStoreError::RetryableStorage(secret.to_string())),
            RpcHostError::from(starweaver_stream::ReplayError::Failed(secret.to_string())),
            RpcHostError::Runtime(secret.to_string()),
            RpcHostError::Io(std::io::Error::other(secret)),
        ];

        for error in cases {
            let rpc = RpcError::from(error);
            assert!(!rpc.message.contains(secret));
        }
    }
}
