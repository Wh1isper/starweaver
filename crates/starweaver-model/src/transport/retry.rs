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
/// Retries are applied while establishing the stream and when the provider reports a 429 before
/// any output-bearing event. Startup events are buffered across this probe and replayed only for
/// the successful attempt. Once an output-bearing event is observed, later stream errors are not
/// replayed.
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
            let stream = client
                .send_websocket_event_stream_incremental(request)
                .await?;
            probe_websocket_rate_limit(stream).await
        }
    })
    .await
}

/// Send a session-scoped WebSocket stream request with retry policy.
///
/// The session is reset between retry attempts so reusable WebSocket state cannot leak across a
/// failed setup attempt or a pre-output 429 response.
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
        let result = match session
            .send_websocket_event_stream_incremental(request.clone())
            .await
        {
            Ok(stream) => probe_websocket_rate_limit(stream).await,
            Err(error) => Err(error),
        };
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

async fn probe_websocket_rate_limit(
    mut stream: ModelEventStream,
) -> Result<ModelEventStream, ModelError> {
    let mut prefetched = Vec::new();
    while let Some(event) = stream.recv().await {
        match event {
            Ok(value) => {
                if let Some(error) = websocket_rate_limit_event_error(&value) {
                    return Err(error);
                }
                let continue_probe = is_websocket_startup_event(&value);
                prefetched.push(Ok(value));
                if !continue_probe {
                    return Ok(stream.prepend_events(prefetched));
                }
            }
            Err(error) => {
                if matches!(error, ModelError::ProviderStatus { status: 429, .. }) {
                    return Err(error);
                }
                prefetched.push(Err(error));
                return Ok(stream.prepend_events(prefetched));
            }
        }
    }
    Ok(stream.prepend_events(prefetched))
}

fn websocket_rate_limit_event_error(event: &serde_json::Value) -> Option<ModelError> {
    let kind = event.get("type").and_then(serde_json::Value::as_str)?;
    if !matches!(kind, "error" | "response.failed") {
        return None;
    }

    let response = event.get("response");
    let error = event
        .get("error")
        .or_else(|| response.and_then(|response| response.get("error")));
    let status = [Some(event), response, error]
        .into_iter()
        .flatten()
        .find_map(websocket_event_status);
    let code = error
        .and_then(|error| error.get("code").or_else(|| error.get("type")))
        .and_then(serde_json::Value::as_str);
    if status != Some(429)
        && !matches!(
            code,
            Some("rate_limit_exceeded" | "insufficient_quota" | "usage_not_included")
        )
    {
        return None;
    }

    Some(ModelError::ProviderStatus {
        status: 429,
        body: event.clone(),
        retryable: true,
    })
}

fn websocket_event_status(value: &serde_json::Value) -> Option<u16> {
    value
        .get("status")
        .or_else(|| value.get("status_code"))
        .and_then(serde_json::Value::as_u64)
        .and_then(|status| u16::try_from(status).ok())
}

fn is_websocket_startup_event(event: &serde_json::Value) -> bool {
    matches!(
        event.get("type").and_then(serde_json::Value::as_str),
        Some("response.created" | "response.in_progress" | "response.queued")
    )
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
    use std::{
        collections::{BTreeMap, VecDeque},
        sync::Mutex,
    };

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
    async fn event_stream_retries_provider_429_setup_errors() {
        let client = RetryStreamClient::rate_limited(1);
        let mut stream = result_or_panic(
            send_event_stream_with_retries(
                &client,
                &NoopSleeper,
                test_request(),
                &test_retry_policy(2),
            )
            .await,
            "SSE setup should retry 429",
        );

        assert_eq!(client.attempts(), 2);
        assert!(matches!(stream.recv().await, Some(Ok(_))));
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

    #[tokio::test]
    async fn websocket_stream_retries_429_before_output() {
        let client = ScriptedWebSocketClient::new(vec![
            vec![
                Ok(json!({"type": "response.created"})),
                Ok(json!({
                    "type": "error",
                    "status": 429,
                    "error": {"type": "usage_limit_reached"}
                })),
            ],
            vec![
                Ok(json!({"type": "response.created"})),
                Ok(json!({"type": "response.completed"})),
            ],
        ]);
        let mut stream = result_or_panic(
            send_websocket_event_stream_with_retries(
                &client,
                &NoopSleeper,
                test_request(),
                &test_retry_policy(3),
            )
            .await,
            "websocket 429 should retry",
        );

        assert_eq!(client.attempts(), 2);
        assert_eq!(
            result_or_panic(
                option_or_panic(stream.recv().await, "created event should be preserved"),
                "created event should succeed",
            ),
            json!({"type": "response.created"})
        );
        assert_eq!(
            result_or_panic(
                option_or_panic(stream.recv().await, "completed event should be preserved"),
                "completed event should succeed",
            ),
            json!({"type": "response.completed"})
        );
        assert!(stream.recv().await.is_none());
    }

    #[tokio::test]
    async fn websocket_session_retries_429_before_output_and_resets() {
        let mut session = ScriptedWebSocketSession::new(vec![
            vec![Err(rate_limit_error())],
            vec![Ok(json!({"type": "response.completed"}))],
        ]);
        let mut stream = result_or_panic(
            send_websocket_session_event_stream_with_retries(
                &mut session,
                &NoopSleeper,
                test_request(),
                &test_retry_policy(2),
            )
            .await,
            "websocket session 429 should retry",
        );

        assert_eq!(session.attempts, 2);
        assert_eq!(session.resets, 1);
        assert!(matches!(stream.recv().await, Some(Ok(_))));
    }

    #[tokio::test]
    async fn websocket_stream_reports_exhausted_pre_output_429() {
        let client = ScriptedWebSocketClient::new(vec![
            vec![Ok(rate_limit_failed_event())],
            vec![Err(rate_limit_error())],
        ]);
        let Err(error) = send_websocket_event_stream_with_retries(
            &client,
            &NoopSleeper,
            test_request(),
            &test_retry_policy(2),
        )
        .await
        else {
            panic!("repeated websocket 429 should exhaust retries");
        };

        assert!(matches!(
            error,
            ModelError::RetryExhausted {
                attempts: 2,
                source,
            } if matches!(source.as_ref(), ModelError::ProviderStatus { status: 429, .. })
        ));
        assert_eq!(client.attempts(), 2);
    }

    #[tokio::test]
    async fn websocket_rate_limit_probe_preserves_cancellation() {
        let cancellation_token = CancellationToken::default();
        let (_sender, receiver) = tokio::sync::mpsc::channel(1);
        let stream = ModelEventStream::new_with_cancellation(receiver, cancellation_token.clone());
        cancellation_token.cancel();

        let mut stream = result_or_panic(
            tokio::time::timeout(
                Duration::from_millis(100),
                probe_websocket_rate_limit(stream),
            )
            .await
            .unwrap_or_else(|_| panic!("cancelled websocket probe should stop waiting")),
            "cancellation should remain a stream error",
        );

        assert!(matches!(
            stream.recv().await,
            Some(Err(ModelError::Cancelled { .. }))
        ));
    }

    #[tokio::test]
    async fn websocket_rate_limit_probe_aborts_discarded_stream() {
        let drop_abort_token = CancellationToken::default();
        let stream = stream_from_events_with_drop_abort(
            vec![Ok(rate_limit_failed_event())],
            drop_abort_token.clone(),
        );

        let Err(error) = probe_websocket_rate_limit(stream).await else {
            panic!("pre-output websocket 429 should fail the attempt");
        };

        assert!(matches!(
            error,
            ModelError::ProviderStatus { status: 429, .. }
        ));
        assert!(drop_abort_token.is_cancelled());
    }

    #[tokio::test]
    async fn websocket_stream_does_not_retry_429_after_output() {
        let client = ScriptedWebSocketClient::new(vec![
            vec![
                Ok(json!({"type": "response.output_text.delta", "delta": "partial"})),
                Err(rate_limit_error()),
            ],
            vec![Ok(json!({"type": "response.completed"}))],
        ]);
        let mut stream = result_or_panic(
            send_websocket_event_stream_with_retries(
                &client,
                &NoopSleeper,
                test_request(),
                &test_retry_policy(3),
            )
            .await,
            "websocket stream should be returned after output starts",
        );

        assert!(matches!(stream.recv().await, Some(Ok(value)) if value["delta"] == "partial"));
        assert!(matches!(
            stream.recv().await,
            Some(Err(ModelError::ProviderStatus { status: 429, .. }))
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
        fail_with_rate_limit: bool,
        stream_error: bool,
    }

    impl RetryStreamClient {
        fn new(fail_setup_attempts: u32, stream_error: bool) -> Self {
            Self {
                attempts: Mutex::new(0),
                fail_setup_attempts,
                fail_with_rate_limit: false,
                stream_error,
            }
        }

        fn rate_limited(fail_setup_attempts: u32) -> Self {
            Self {
                attempts: Mutex::new(0),
                fail_setup_attempts,
                fail_with_rate_limit: true,
                stream_error: false,
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
                return if self.fail_with_rate_limit {
                    Err(rate_limit_error())
                } else {
                    Err(ModelError::Transport("setup failure".to_string()))
                };
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

    #[derive(Debug)]
    struct ScriptedWebSocketClient {
        attempts: Mutex<u32>,
        streams: Mutex<VecDeque<Vec<Result<serde_json::Value, ModelError>>>>,
    }

    impl ScriptedWebSocketClient {
        fn new(streams: Vec<Vec<Result<serde_json::Value, ModelError>>>) -> Self {
            Self {
                attempts: Mutex::new(0),
                streams: Mutex::new(VecDeque::from(streams)),
            }
        }

        fn attempts(&self) -> u32 {
            *lock_or_panic(self.attempts.lock(), "attempts lock should not be poisoned")
        }
    }

    #[async_trait]
    impl ModelHttpClient for ScriptedWebSocketClient {
        async fn send(&self, _request: HttpRequest) -> Result<HttpResponse, ModelError> {
            Err(ModelError::Transport(
                "send is not used by websocket retry tests".to_string(),
            ))
        }

        async fn send_websocket_event_stream_incremental(
            &self,
            _request: HttpRequest,
        ) -> Result<ModelEventStream, ModelError> {
            *lock_or_panic(self.attempts.lock(), "attempts lock should not be poisoned") += 1;
            let events = lock_or_panic(self.streams.lock(), "streams lock should not be poisoned")
                .pop_front()
                .ok_or_else(|| {
                    ModelError::Transport("missing scripted websocket stream".to_string())
                })?;
            Ok(stream_from_events(events))
        }
    }

    #[derive(Debug)]
    struct ScriptedWebSocketSession {
        attempts: u32,
        resets: u32,
        streams: VecDeque<Vec<Result<serde_json::Value, ModelError>>>,
    }

    impl ScriptedWebSocketSession {
        fn new(streams: Vec<Vec<Result<serde_json::Value, ModelError>>>) -> Self {
            Self {
                attempts: 0,
                resets: 0,
                streams: VecDeque::from(streams),
            }
        }
    }

    #[async_trait]
    impl ModelWebSocketEventSession for ScriptedWebSocketSession {
        async fn send_websocket_event_stream_incremental(
            &mut self,
            _request: HttpRequest,
        ) -> Result<ModelEventStream, ModelError> {
            self.attempts += 1;
            let events = self.streams.pop_front().ok_or_else(|| {
                ModelError::Transport("missing scripted websocket session stream".to_string())
            })?;
            Ok(stream_from_events(events))
        }

        async fn reset(&mut self) {
            self.resets += 1;
        }
    }

    fn stream_from_events(events: Vec<Result<serde_json::Value, ModelError>>) -> ModelEventStream {
        let (sender, receiver) = tokio::sync::mpsc::channel(8);
        tokio::spawn(async move {
            for event in events {
                if sender.send(event).await.is_err() {
                    return;
                }
            }
        });
        ModelEventStream::new(receiver)
    }

    fn stream_from_events_with_drop_abort(
        events: Vec<Result<serde_json::Value, ModelError>>,
        drop_abort_token: CancellationToken,
    ) -> ModelEventStream {
        let (sender, receiver) = tokio::sync::mpsc::channel(8);
        tokio::spawn(async move {
            for event in events {
                if sender.send(event).await.is_err() {
                    return;
                }
            }
        });
        ModelEventStream::new_with_cancellation_and_drop_abort(
            receiver,
            CancellationToken::default(),
            Some(drop_abort_token),
        )
    }

    fn rate_limit_failed_event() -> serde_json::Value {
        json!({
            "type": "response.failed",
            "response": {
                "status": "failed",
                "error": {"code": "rate_limit_exceeded"}
            }
        })
    }

    fn rate_limit_error() -> ModelError {
        ModelError::ProviderStatus {
            status: 429,
            body: json!({"error": "rate limited"}),
            retryable: true,
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
