//! Capability error types.

use starweaver_model::ModelResponse;
use thiserror::Error;

/// Runtime capability error.
#[derive(Debug, Error)]
pub enum CapabilityError {
    /// Ask the model to retry with this prompt.
    #[error("model retry requested: {0}")]
    ModelRetry(String),
    /// Return this response without calling the model.
    #[error("model request skipped")]
    SkipModelRequest(Box<ModelResponse>),
    /// Capability hook failed.
    #[error("capability failed: {0}")]
    Failed(String),
}

/// Capability hook result.
pub type CapabilityResult<T> = Result<T, CapabilityError>;
