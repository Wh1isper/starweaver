use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::ModelError;

use super::{HttpRequest, HttpResponse, ModelHttpClient};

/// Async sleep abstraction used by retry policies.
#[async_trait]
pub trait ModelSleeper: Send + Sync {
    /// Sleep for the provided duration.
    async fn sleep(&self, duration: Duration);
}

/// Tokio-backed sleeper.
#[derive(Clone, Debug, Default)]
pub struct TokioSleeper;

#[async_trait]
impl ModelSleeper for TokioSleeper {
    async fn sleep(&self, duration: Duration) {
        tokio::time::sleep(duration).await;
    }
}

/// Sleeper that returns immediately, useful for deterministic tests.
#[derive(Clone, Debug, Default)]
pub struct NoopSleeper;

#[async_trait]
impl ModelSleeper for NoopSleeper {
    async fn sleep(&self, _duration: Duration) {}
}

/// Shared reference to a sleeper.
pub type DynSleeper = Arc<dyn ModelSleeper>;

/// Retry policy for transient model transport failures.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RetryPolicy {
    /// Maximum number of attempts including the first attempt.
    pub max_attempts: u32,
    /// Base delay in milliseconds for exponential backoff.
    pub base_delay_ms: u64,
    /// Maximum delay in milliseconds.
    pub max_delay_ms: u64,
    /// Retry HTTP status codes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retry_statuses: Vec<u16>,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_delay_ms: 250,
            max_delay_ms: 2_000,
            retry_statuses: vec![408, 409, 425, 429, 500, 502, 503, 504],
        }
    }
}

impl RetryPolicy {
    /// Return the delay for an attempt index starting at one.
    #[must_use]
    pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
        let exponent = attempt.saturating_sub(1).min(31);
        let multiplier = 1_u64.checked_shl(exponent).unwrap_or(u64::MAX);
        Duration::from_millis(
            self.base_delay_ms
                .saturating_mul(multiplier)
                .min(self.max_delay_ms),
        )
    }

    /// Return whether this policy should retry a status.
    #[must_use]
    pub fn retries_status(&self, status: u16) -> bool {
        self.retry_statuses.contains(&status)
    }
}

/// Return whether a status is commonly retryable.
#[must_use]
pub fn is_retryable_status(status: u16) -> bool {
    RetryPolicy::default().retries_status(status)
}

/// Return whether a model error is retryable under the provided policy.
#[must_use]
pub fn should_retry_error(error: &ModelError, policy: &RetryPolicy) -> bool {
    match error {
        ModelError::Transport(_) => true,
        ModelError::ProviderStatus {
            status, retryable, ..
        } => *retryable || policy.retries_status(*status),
        ModelError::RetryExhausted { .. }
        | ModelError::Cancelled { .. }
        | ModelError::RealModelRequestBlocked { .. }
        | ModelError::MessageMapping(_)
        | ModelError::ResponseParsing(_)
        | ModelError::UnsupportedResponse(_) => false,
    }
}

/// Send a request with retry policy.
///
/// # Errors
///
/// Returns the final transport error or retry exhaustion error.
pub async fn send_with_retries(
    client: &dyn ModelHttpClient,
    sleeper: &dyn ModelSleeper,
    request: HttpRequest,
    policy: &RetryPolicy,
) -> Result<HttpResponse, ModelError> {
    let max_attempts = policy.max_attempts.max(1);
    let mut attempt = 1;
    loop {
        let result = client.send(request.clone()).await;
        match result {
            Ok(response) => return Ok(response),
            Err(error) if attempt < max_attempts && should_retry_error(&error, policy) => {
                sleeper.sleep(policy.delay_for_attempt(attempt)).await;
                attempt += 1;
            }
            Err(error) if attempt >= max_attempts && should_retry_error(&error, policy) => {
                return Err(ModelError::RetryExhausted {
                    attempts: attempt,
                    source: Box::new(error),
                });
            }
            Err(error) => return Err(error),
        }
    }
}
