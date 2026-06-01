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
    /// Store failed.
    #[error("session store failed: {0}")]
    Failed(String),
}

/// Result alias for session store operations.
pub type SessionStoreResult<T> = Result<T, SessionStoreError>;

impl From<SessionStoreError> for starweaver_runtime::AgentExecutorError {
    fn from(error: SessionStoreError) -> Self {
        Self::Failed(error.to_string())
    }
}
