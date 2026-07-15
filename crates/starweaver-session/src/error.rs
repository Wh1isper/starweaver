//! Session store errors.

use thiserror::Error;

/// Session store failure.
#[derive(Debug, Error)]
pub enum SessionStoreError {
    /// Record was not found.
    #[error("session record not found: {0}")]
    NotFound(String),
    /// Record already exists.
    #[error("session record already exists: {0}")]
    AlreadyExists(String),
    /// Optimistic revision or lifecycle conflict.
    #[error("session record conflict: {0}")]
    Conflict(String),
    /// One idempotency key was reused for a different normalized command.
    #[error("session idempotency conflict: {0}")]
    IdempotencyConflict(String),
    /// A trusted host retention or admission quota was exceeded.
    #[error("session quota exceeded: {0}")]
    QuotaExceeded(String),
    /// The session already owns a live run admission.
    #[error("session run conflict: {0}")]
    RunConflict(String),
    /// Store failed.
    #[error("session store failed: {0}")]
    Failed(String),
}

/// Result alias for session store operations.
pub type SessionStoreResult<T> = Result<T, SessionStoreError>;

impl From<SessionStoreError> for starweaver_context::AgentExecutorError {
    fn from(error: SessionStoreError) -> Self {
        Self::Failed(error.to_string())
    }
}
