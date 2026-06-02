//! HTTP transport boundary for production model adapters.

use std::{collections::BTreeMap, sync::Arc, time::Duration};

use async_trait::async_trait;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{allow_real_model_requests, ModelError};

/// Stable HTTP method set required by model transports.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    /// HTTP POST.
    Post,
}

/// Request sent to an injected model HTTP client.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HttpRequest {
    /// HTTP method.
    pub method: HttpMethod,
    /// Absolute endpoint URL.
    pub url: String,
    /// Headers after adapter defaults, provider config, and request overrides are merged.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    /// JSON request body after extra body merge.
    pub body: Value,
    /// Optional request timeout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<Duration>,
    /// Request metadata for tracing and auditing.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub metadata: Map<String, Value>,
}

/// Response returned by an injected model HTTP client.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HttpResponse {
    /// HTTP status code.
    pub status: u16,
    /// Response headers.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    /// JSON response body.
    pub body: Value,
}

impl HttpResponse {
    /// Return a successful JSON response.
    #[must_use]
    pub const fn ok(body: Value) -> Self {
        Self {
            status: 200,
            headers: BTreeMap::new(),
            body,
        }
    }
}

/// Max-token request parameter mapping for provider or gateway HTTP configs.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MaxTokensParameter {
    /// Use the protocol adapter's default mapping.
    #[default]
    Default,
    /// Emit `max_tokens`.
    MaxTokens,
    /// Emit `max_output_tokens`.
    MaxOutputTokens,
    /// Omit provider max-token fields.
    Omit,
}

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

/// Async HTTP client abstraction used by production model adapters.
#[async_trait]
pub trait ModelHttpClient: Send + Sync {
    /// Send a JSON model request.
    ///
    /// # Errors
    ///
    /// Returns an error when transport, status, or response decoding fails.
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, ModelError>;

    /// Send a server-sent events model request and return JSON `data:` payloads.
    ///
    /// # Errors
    ///
    /// Returns an error when transport, status, or event decoding fails.
    async fn send_event_stream(&self, request: HttpRequest) -> Result<Vec<Value>, ModelError> {
        Err(ModelError::Transport(format!(
            "server-sent event streaming is not implemented for {}",
            request.url
        )))
    }
}

/// Shared reference to an HTTP client.
pub type DynHttpClient = Arc<dyn ModelHttpClient>;

/// Reqwest-backed HTTP client.
#[derive(Clone, Debug)]
pub struct ReqwestHttpClient {
    client: reqwest::Client,
}

impl ReqwestHttpClient {
    /// Create a reqwest-backed client with rustls TLS.
    ///
    /// # Errors
    ///
    /// Returns an error when reqwest client construction fails.
    pub fn new() -> Result<Self, ModelError> {
        let client = reqwest::Client::builder()
            .build()
            .map_err(|err| ModelError::Transport(err.to_string()))?;
        Ok(Self { client })
    }

    async fn send_request(&self, request: &HttpRequest) -> Result<reqwest::Response, ModelError> {
        if !allow_real_model_requests() {
            return Err(ModelError::RealModelRequestBlocked {
                url: request.url.clone(),
            });
        }

        let mut builder = match request.method {
            HttpMethod::Post => self.client.post(&request.url),
        }
        .headers(Self::header_map(&request.headers)?)
        .json(&request.body);

        if let Some(timeout) = request.timeout {
            builder = builder.timeout(timeout);
        }

        builder
            .send()
            .await
            .map_err(|err| ModelError::Transport(err.to_string()))
    }

    fn header_map(headers: &BTreeMap<String, String>) -> Result<HeaderMap, ModelError> {
        let mut map = HeaderMap::new();
        for (name, value) in headers {
            let name = HeaderName::from_bytes(name.as_bytes()).map_err(|err| {
                ModelError::Transport(format!("invalid header name {name}: {err}"))
            })?;
            let value = HeaderValue::from_str(value).map_err(|err| {
                ModelError::Transport(format!("invalid header value for {name}: {err}"))
            })?;
            map.insert(name, value);
        }
        Ok(map)
    }
}

#[async_trait]
impl ModelHttpClient for ReqwestHttpClient {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, ModelError> {
        let response = self.send_request(&request).await?;
        let status = response.status().as_u16();
        let headers = response_headers(&response);
        let body = response
            .json::<Value>()
            .await
            .map_err(|err| ModelError::Transport(err.to_string()))?;

        if (200..300).contains(&status) {
            Ok(HttpResponse {
                status,
                headers,
                body,
            })
        } else {
            Err(ModelError::ProviderStatus {
                status,
                body,
                retryable: is_retryable_status(status),
            })
        }
    }

    async fn send_event_stream(&self, request: HttpRequest) -> Result<Vec<Value>, ModelError> {
        let response = self.send_request(&request).await?;
        let status = response.status().as_u16();
        let text = response
            .text()
            .await
            .map_err(|err| ModelError::Transport(err.to_string()))?;
        if !(200..300).contains(&status) {
            let body = serde_json::from_str(&text).unwrap_or(Value::String(text));
            return Err(ModelError::ProviderStatus {
                status,
                body,
                retryable: is_retryable_status(status),
            });
        }
        parse_sse_json_events(&text)
    }
}

fn response_headers(response: &reqwest::Response) -> BTreeMap<String, String> {
    response
        .headers()
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_string(), value.to_string()))
        })
        .collect()
}

fn parse_sse_json_events(text: &str) -> Result<Vec<Value>, ModelError> {
    let mut events = Vec::new();
    let mut data_lines = Vec::new();
    for line in text.lines() {
        if let Some(data) = line.strip_prefix("data:") {
            data_lines.push(data.trim_start().to_string());
            continue;
        }
        if line.trim().is_empty() && !data_lines.is_empty() {
            push_sse_json_event(&mut events, &data_lines)?;
            data_lines.clear();
        }
    }
    if !data_lines.is_empty() {
        push_sse_json_event(&mut events, &data_lines)?;
    }
    Ok(events)
}

fn push_sse_json_event(events: &mut Vec<Value>, data_lines: &[String]) -> Result<(), ModelError> {
    let data = data_lines.join("\n");
    if data.trim() == "[DONE]" {
        return Ok(());
    }
    let value = serde_json::from_str::<Value>(&data).map_err(|error| {
        ModelError::ResponseParsing(format!("invalid server-sent event JSON: {error}"))
    })?;
    events.push(value);
    Ok(())
}

/// Authentication strategy for HTTP model adapters.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AuthConfig {
    /// Bearer token sent through the `Authorization` header.
    Bearer {
        /// Token value.
        token: String,
    },
    /// API key sent through a named header.
    Header {
        /// Header name.
        name: String,
        /// Header value.
        value: String,
    },
}

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

/// Provider HTTP configuration shared by protocol clients.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HttpModelConfig {
    /// Provider or gateway base URL.
    pub base_url: String,
    /// Provider-specific endpoint path.
    pub endpoint_path: String,
    /// Authentication config.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<AuthConfig>,
    /// Headers applied to all requests.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    /// Extra JSON body merged into every provider request.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub extra_body: Map<String, Value>,
    /// Default timeout in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// Retry policy for transient failures.
    #[serde(default)]
    pub retry_policy: RetryPolicy,
    /// Provider or gateway max-token parameter mapping.
    #[serde(default)]
    pub max_tokens_parameter: MaxTokensParameter,
    /// Adapter-level metadata copied into every request.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub metadata: Map<String, Value>,
}

impl HttpModelConfig {
    /// Create HTTP config from base URL and endpoint path.
    #[must_use]
    pub fn new(base_url: impl Into<String>, endpoint_path: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            endpoint_path: endpoint_path.into(),
            auth: None,
            headers: BTreeMap::new(),
            extra_body: Map::new(),
            timeout_ms: None,
            retry_policy: RetryPolicy::default(),
            max_tokens_parameter: MaxTokensParameter::Default,
            metadata: Map::new(),
        }
    }

    /// Resolve the absolute endpoint URL.
    #[must_use]
    pub fn endpoint_url(&self) -> String {
        let base = self.base_url.trim_end_matches('/');
        let path = self.endpoint_path.trim_start_matches('/');
        format!("{base}/{path}")
    }
}

/// Per-request HTTP overrides for gateway, audit, and routing use cases.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct HttpRequestOptions {
    /// Headers applied to this request.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    /// Extra JSON body merged into this request.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub extra_body: Map<String, Value>,
    /// Endpoint override for this request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint_url: Option<String>,
    /// Timeout override in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// Request metadata for tracing and auditing.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub metadata: Map<String, Value>,
}

/// Merge extra JSON body object into a provider request object.
#[must_use]
pub fn merge_extra_body(mut body: Value, extra: &Map<String, Value>) -> Value {
    if let Value::Object(object) = &mut body {
        for (key, value) in extra {
            object.insert(key.clone(), value.clone());
        }
    }
    body
}

fn merge_metadata(config: &HttpModelConfig, options: &HttpRequestOptions) -> Map<String, Value> {
    let mut metadata = config.metadata.clone();
    metadata.extend(options.metadata.clone());
    metadata
}

/// Build a concrete HTTP request from provider config and overrides.
#[must_use]
pub fn build_http_request(
    config: &HttpModelConfig,
    options: &HttpRequestOptions,
    body: Value,
) -> HttpRequest {
    let mut headers = BTreeMap::from([(
        CONTENT_TYPE.as_str().to_string(),
        "application/json".to_string(),
    )]);

    match &config.auth {
        Some(AuthConfig::Bearer { token }) => {
            headers.insert(
                AUTHORIZATION.as_str().to_string(),
                format!("Bearer {token}"),
            );
        }
        Some(AuthConfig::Header { name, value }) => {
            headers.insert(name.clone(), value.clone());
        }
        None => {}
    }

    headers.extend(config.headers.clone());
    headers.extend(options.headers.clone());

    let body = merge_extra_body(
        merge_extra_body(body, &config.extra_body),
        &options.extra_body,
    );
    let timeout_ms = options.timeout_ms.or(config.timeout_ms);

    HttpRequest {
        method: HttpMethod::Post,
        url: options
            .endpoint_url
            .clone()
            .unwrap_or_else(|| config.endpoint_url()),
        headers,
        body,
        timeout: timeout_ms.map(Duration::from_millis),
        metadata: merge_metadata(config, options),
    }
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
