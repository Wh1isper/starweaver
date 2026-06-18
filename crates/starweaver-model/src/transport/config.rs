use std::{collections::BTreeMap, time::Duration};

use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{HttpMethod, HttpRequest, MaxTokensParameter, RetryPolicy};

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

/// Extend HTTP headers using case-insensitive header-name replacement.
pub fn extend_headers_case_insensitive(
    headers: &mut BTreeMap<String, String>,
    overlay: impl IntoIterator<Item = (String, String)>,
) {
    for (key, value) in overlay {
        headers.retain(|existing, _| !existing.eq_ignore_ascii_case(&key));
        headers.insert(key, value);
    }
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
            extend_headers_case_insensitive(
                &mut headers,
                [(
                    AUTHORIZATION.as_str().to_string(),
                    format!("Bearer {token}"),
                )],
            );
        }
        Some(AuthConfig::Header { name, value }) => {
            extend_headers_case_insensitive(&mut headers, [(name.clone(), value.clone())]);
        }
        None => {}
    }

    extend_headers_case_insensitive(&mut headers, config.headers.clone());
    extend_headers_case_insensitive(&mut headers, options.headers.clone());

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
        cancellation_token: starweaver_core::CancellationToken::default(),
    }
}
