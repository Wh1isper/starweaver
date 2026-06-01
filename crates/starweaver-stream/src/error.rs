//! Replay and stream archive errors.

use thiserror::Error;

/// Replay and stream failure.
#[derive(Debug, Error)]
pub enum ReplayError {
    /// Record was not found.
    #[error("stream record not found: {0}")]
    NotFound(String),
    /// Cursor is invalid for the requested scope.
    #[error("invalid replay cursor: {0}")]
    InvalidCursor(String),
    /// Stream operation failed.
    #[error("stream operation failed: {0}")]
    Failed(String),
}

/// Result alias for replay and stream operations.
pub type ReplayResult<T> = Result<T, ReplayError>;
