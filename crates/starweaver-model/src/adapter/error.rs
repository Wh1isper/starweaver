use serde_json::Value;
use thiserror::Error;

/// Model adapter error.
#[derive(Debug, Error)]
pub enum ModelError {
    /// Canonical history cannot be mapped into a provider request.
    #[error("message mapping failed: {0}")]
    MessageMapping(String),
    /// Provider response cannot be parsed into canonical response.
    #[error("response parsing failed: {0}")]
    ResponseParsing(String),
    /// Transport failed.
    #[error("transport failed: {0}")]
    Transport(String),
    /// A real HTTP model request was blocked by the global test guard.
    #[error("real model request blocked for {url}")]
    RealModelRequestBlocked {
        /// Target request URL.
        url: String,
    },
    /// Provider returned a non-success status.
    #[error("provider status {status}: {body}")]
    ProviderStatus {
        /// HTTP status code.
        status: u16,
        /// Provider response body.
        body: Value,
        /// Whether retry policy may retry this status.
        retryable: bool,
    },
    /// Retry attempts were exhausted.
    #[error("retry attempts exhausted after {attempts} attempts: {source}")]
    RetryExhausted {
        /// Attempt count.
        attempts: u32,
        /// Last error.
        source: Box<Self>,
    },
    /// Provider returned an unsupported response shape.
    #[error("unsupported provider response: {0}")]
    UnsupportedResponse(String),
}
