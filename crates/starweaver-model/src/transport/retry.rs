use std::{future::Future, sync::Arc, time::Duration};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use starweaver_core::CancellationToken;

use crate::ModelError;

use super::{
    HttpRequest, HttpResponse, ModelEventStream, ModelHttpClient, ModelWebSocketEventSession,
};

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
            max_attempts: 5,
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

/// Run a fallible model transport operation with retry policy.
///
/// The operation is retried only when it returns a retryable setup/request error. Once an
/// operation succeeds and hands a response or stream back to the caller, later stream-consumption
/// errors are not replayed by this helper.
///
/// # Errors
///
/// Returns the final non-retryable error, a cancellation error, or retry exhaustion error.
pub async fn retry_with_policy<T, Fut, Operation>(
    sleeper: &dyn ModelSleeper,
    policy: &RetryPolicy,
    cancellation_token: Option<CancellationToken>,
    mut operation: Operation,
) -> Result<T, ModelError>
where
    Fut: Future<Output = Result<T, ModelError>>,
    Operation: FnMut() -> Fut,
{
    let max_attempts = policy.max_attempts.max(1);
    let mut attempt = 1;
    loop {
        let result = operation().await;
        match result {
            Ok(value) => return Ok(value),
            Err(error) if attempt < max_attempts && should_retry_error(&error, policy) => {
                sleep_before_retry(
                    sleeper,
                    policy.delay_for_attempt(attempt),
                    cancellation_token.as_ref(),
                )
                .await?;
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

/// Send a JSON request with retry policy.
///
/// # Errors
///
/// Returns the final transport error, a cancellation error, or retry exhaustion error.
pub async fn send_with_retries(
    client: &dyn ModelHttpClient,
    sleeper: &dyn ModelSleeper,
    request: HttpRequest,
    policy: &RetryPolicy,
) -> Result<HttpResponse, ModelError> {
    let cancellation_token = request.cancellation_token.clone();
    retry_with_policy(sleeper, policy, Some(cancellation_token), || {
        let request = request.clone();
        async move { client.send(request).await }
    })
    .await
}

/// Send an SSE request with retry policy.
///
/// Retries are applied only while establishing the stream. Once a stream is returned, event
/// consumption is not replayed to avoid duplicating provider-side work or canonical events.
///
/// # Errors
///
/// Returns the final setup error, a cancellation error, or retry exhaustion error.
pub async fn send_event_stream_with_retries(
    client: &dyn ModelHttpClient,
    sleeper: &dyn ModelSleeper,
    request: HttpRequest,
    policy: &RetryPolicy,
) -> Result<ModelEventStream, ModelError> {
    let cancellation_token = request.cancellation_token.clone();
    retry_with_policy(sleeper, policy, Some(cancellation_token), || {
        let request = request.clone();
        async move { client.send_event_stream_incremental(request).await }
    })
    .await
}

/// Send a per-request WebSocket stream request with retry policy.
///
/// Retries are applied only while establishing the stream. Once a stream is returned, event
/// consumption is not replayed.
///
/// # Errors
///
/// Returns the final setup error, a cancellation error, or retry exhaustion error.
pub async fn send_websocket_event_stream_with_retries(
    client: &dyn ModelHttpClient,
    sleeper: &dyn ModelSleeper,
    request: HttpRequest,
    policy: &RetryPolicy,
) -> Result<ModelEventStream, ModelError> {
    let cancellation_token = request.cancellation_token.clone();
    retry_with_policy(sleeper, policy, Some(cancellation_token), || {
        let request = request.clone();
        async move {
            client
                .send_websocket_event_stream_incremental(request)
                .await
        }
    })
    .await
}

/// Send a session-scoped WebSocket stream request with retry policy.
///
/// The session is reset between retry attempts so reusable WebSocket state cannot leak across a
/// failed setup attempt.
///
/// # Errors
///
/// Returns the final setup error, a cancellation error, or retry exhaustion error.
pub async fn send_websocket_session_event_stream_with_retries(
    session: &mut dyn ModelWebSocketEventSession,
    sleeper: &dyn ModelSleeper,
    request: HttpRequest,
    policy: &RetryPolicy,
) -> Result<ModelEventStream, ModelError> {
    let max_attempts = policy.max_attempts.max(1);
    let mut attempt = 1;
    loop {
        let result = session
            .send_websocket_event_stream_incremental(request.clone())
            .await;
        match result {
            Ok(stream) => return Ok(stream),
            Err(error) if attempt < max_attempts && should_retry_error(&error, policy) => {
                session.reset().await;
                sleep_before_retry(
                    sleeper,
                    policy.delay_for_attempt(attempt),
                    Some(&request.cancellation_token),
                )
                .await?;
                attempt += 1;
            }
            Err(error) if attempt >= max_attempts && should_retry_error(&error, policy) => {
                session.reset().await;
                return Err(ModelError::RetryExhausted {
                    attempts: attempt,
                    source: Box::new(error),
                });
            }
            Err(error) => return Err(error),
        }
    }
}

async fn sleep_before_retry(
    sleeper: &dyn ModelSleeper,
    duration: Duration,
    cancellation_token: Option<&CancellationToken>,
) -> Result<(), ModelError> {
    let Some(cancellation_token) = cancellation_token else {
        if !duration.is_zero() {
            sleeper.sleep(duration).await;
        }
        return Ok(());
    };
    if cancellation_token.is_cancelled() {
        return Err(ModelError::Cancelled {
            reason: "model request cancellation requested before retry".to_string(),
        });
    }
    if duration.is_zero() {
        return Ok(());
    }
    tokio::select! {
        biased;
        () = cancellation_token.cancelled() => Err(ModelError::Cancelled {
            reason: "model request cancellation requested before retry".to_string(),
        }),
        () = sleeper.sleep(duration) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, sync::Mutex};

    use async_trait::async_trait;
    use serde_json::{Map, json};

    use super::*;
    use crate::transport::{HttpMethod, HttpRequest};

    #[tokio::test]
    async fn event_stream_retries_setup_errors() {
        let client = RetryStreamClient::new(2, false);
        let mut stream = result_or_panic(
            send_event_stream_with_retries(
                &client,
                &NoopSleeper,
                test_request(),
                &test_retry_policy(3),
            )
            .await,
            "stream setup should eventually succeed",
        );

        assert_eq!(client.attempts(), 3);
        assert_eq!(
            result_or_panic(
                option_or_panic(stream.recv().await, "stream event should exist"),
                "event should succeed",
            ),
            json!({"type": "response.completed"})
        );
    }

    #[tokio::test]
    async fn event_stream_does_not_retry_after_stream_is_returned() {
        let client = RetryStreamClient::new(0, true);
        let mut stream = result_or_panic(
            send_event_stream_with_retries(
                &client,
                &NoopSleeper,
                test_request(),
                &test_retry_policy(3),
            )
            .await,
            "stream setup should succeed",
        );

        assert_eq!(client.attempts(), 1);
        assert!(matches!(
            option_or_panic(stream.recv().await, "stream error should exist"),
            Err(ModelError::Transport(message)) if message == "midstream failure"
        ));
        assert_eq!(client.attempts(), 1);
    }

    fn test_retry_policy(max_attempts: u32) -> RetryPolicy {
        RetryPolicy {
            max_attempts,
            base_delay_ms: 0,
            max_delay_ms: 0,
            retry_statuses: vec![429, 500, 502, 503, 504],
        }
    }

    fn test_request() -> HttpRequest {
        HttpRequest {
            method: HttpMethod::Post,
            url: "https://example.test/model".to_string(),
            headers: BTreeMap::default(),
            body: json!({"input": "hello"}),
            timeout: None,
            metadata: Map::default(),
            cancellation_token: CancellationToken::default(),
        }
    }

    #[derive(Debug)]
    struct RetryStreamClient {
        attempts: Mutex<u32>,
        fail_setup_attempts: u32,
        stream_error: bool,
    }

    impl RetryStreamClient {
        fn new(fail_setup_attempts: u32, stream_error: bool) -> Self {
            Self {
                attempts: Mutex::new(0),
                fail_setup_attempts,
                stream_error,
            }
        }

        fn attempts(&self) -> u32 {
            *lock_or_panic(self.attempts.lock(), "attempts lock should not be poisoned")
        }
    }

    #[async_trait]
    impl ModelHttpClient for RetryStreamClient {
        async fn send(&self, _request: HttpRequest) -> Result<HttpResponse, ModelError> {
            Err(ModelError::Transport(
                "send is not used by retry tests".to_string(),
            ))
        }

        async fn send_event_stream_incremental(
            &self,
            _request: HttpRequest,
        ) -> Result<ModelEventStream, ModelError> {
            let attempt = {
                let mut attempts =
                    lock_or_panic(self.attempts.lock(), "attempts lock should not be poisoned");
                *attempts += 1;
                *attempts
            };
            if attempt <= self.fail_setup_attempts {
                return Err(ModelError::Transport("setup failure".to_string()));
            }

            let (sender, receiver) = tokio::sync::mpsc::channel(4);
            let stream_error = self.stream_error;
            tokio::spawn(async move {
                let event = if stream_error {
                    Err(ModelError::Transport("midstream failure".to_string()))
                } else {
                    Ok(json!({"type": "response.completed"}))
                };
                let _ = sender.send(event).await;
            });
            Ok(ModelEventStream::new(receiver))
        }
    }

    fn result_or_panic<T, E: std::fmt::Debug>(result: Result<T, E>, message: &str) -> T {
        match result {
            Ok(value) => value,
            Err(error) => panic!("{message}: {error:?}"),
        }
    }

    fn option_or_panic<T>(value: Option<T>, message: &str) -> T {
        value.unwrap_or_else(|| panic!("{message}"))
    }

    fn lock_or_panic<'a, T>(
        lock: std::sync::LockResult<std::sync::MutexGuard<'a, T>>,
        message: &str,
    ) -> std::sync::MutexGuard<'a, T> {
        lock.unwrap_or_else(|error| panic!("{message}: {error}"))
    }
}
