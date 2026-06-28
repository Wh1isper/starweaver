//! OAuth bearer HTTP client wrapper.

use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use serde_json::Value;
use starweaver_oauth::OAuthTokenSource;

use crate::{
    oauth::headers::{
        build_codex_headers, patch_codex_responses_body, trace_session_headers,
        validate_safe_extra_headers, CODEX_USER_AGENT_HEADER,
    },
    transport::{
        extend_headers_case_insensitive, DynHttpClient, HttpRequest, HttpResponse,
        ModelEventStream, ModelHttpClient, ReqwestHttpClient,
    },
    ModelError,
};

/// HTTP client wrapper that attaches OAuth bearer headers and refreshes once on 401.
pub struct OAuthBearerHttpClient {
    inner: DynHttpClient,
    token_source: Arc<dyn OAuthTokenSource>,
    provider_name: String,
    extra_headers: BTreeMap<String, String>,
}

impl OAuthBearerHttpClient {
    /// Create a wrapper around an injected model HTTP client.
    ///
    /// # Errors
    ///
    /// Returns an error when `extra_headers` contains reserved OAuth/Codex headers.
    pub fn new(
        inner: DynHttpClient,
        token_source: Arc<dyn OAuthTokenSource>,
        provider_name: impl Into<String>,
        extra_headers: BTreeMap<String, String>,
    ) -> Result<Self, ModelError> {
        validate_safe_extra_headers(&extra_headers)?;
        Ok(Self {
            inner,
            token_source,
            provider_name: provider_name.into(),
            extra_headers,
        })
    }

    /// Create a wrapper using the default reqwest HTTP client.
    ///
    /// # Errors
    ///
    /// Returns an error when reqwest client construction fails or headers are invalid.
    pub fn with_default_http_client(
        token_source: Arc<dyn OAuthTokenSource>,
        provider_name: impl Into<String>,
        extra_headers: BTreeMap<String, String>,
    ) -> Result<Self, ModelError> {
        Self::new(
            Arc::new(ReqwestHttpClient::new()?),
            token_source,
            provider_name,
            extra_headers,
        )
    }

    fn prepare_request(
        &self,
        mut request: HttpRequest,
        snapshot: &starweaver_oauth::TokenSnapshot,
    ) -> Result<HttpRequest, ModelError> {
        let explicit_codex_routing_headers = if self.provider_name == "codex" {
            extract_case_insensitive_headers(&request.headers, CODEX_ROUTING_HEADER_NAMES)
        } else {
            BTreeMap::new()
        };
        request.headers.insert(
            "Authorization".to_string(),
            format!("Bearer {}", snapshot.access_token),
        );
        if self.provider_name == "codex" {
            insert_header_if_absent_case_insensitive(
                &mut request.headers,
                CODEX_USER_AGENT_HEADER,
                codex_user_agent(),
            );
            let explicit_extra_routing_headers =
                extract_case_insensitive_headers(&self.extra_headers, CODEX_ROUTING_HEADER_NAMES);
            let mut extra_headers = trace_session_headers(&request);
            extend_headers_case_insensitive(&mut extra_headers, self.extra_headers.clone());
            restore_case_insensitive_headers(&mut extra_headers, explicit_extra_routing_headers);
            extend_headers_case_insensitive(
                &mut request.headers,
                build_codex_headers(&snapshot.account, Some(&extra_headers))?,
            );
            restore_case_insensitive_headers(&mut request.headers, explicit_codex_routing_headers);
            patch_codex_responses_body(&mut request);
        } else {
            extend_headers_case_insensitive(&mut request.headers, self.extra_headers.clone());
        }
        Ok(request)
    }
}

#[async_trait]
impl ModelHttpClient for OAuthBearerHttpClient {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, ModelError> {
        let snapshot = self
            .token_source
            .get_token()
            .await
            .map_err(|error| ModelError::Transport(error.to_string()))?;
        let request_with_auth = self.prepare_request(request.clone(), &snapshot)?;
        match self.inner.send(request_with_auth).await {
            Err(ModelError::ProviderStatus { status: 401, .. }) => {
                let refreshed = self
                    .token_source
                    .refresh_token()
                    .await
                    .map_err(|error| ModelError::Transport(error.to_string()))?;
                self.inner
                    .send(self.prepare_request(request, &refreshed)?)
                    .await
            }
            result => result,
        }
    }

    async fn send_event_stream(&self, request: HttpRequest) -> Result<Vec<Value>, ModelError> {
        let snapshot = self
            .token_source
            .get_token()
            .await
            .map_err(|error| ModelError::Transport(error.to_string()))?;
        let request_with_auth = self.prepare_request(request.clone(), &snapshot)?;
        match self.inner.send_event_stream(request_with_auth).await {
            Err(ModelError::ProviderStatus { status: 401, .. }) => {
                let refreshed = self
                    .token_source
                    .refresh_token()
                    .await
                    .map_err(|error| ModelError::Transport(error.to_string()))?;
                self.inner
                    .send_event_stream(self.prepare_request(request, &refreshed)?)
                    .await
            }
            result => result,
        }
    }

    async fn send_event_stream_incremental(
        &self,
        request: HttpRequest,
    ) -> Result<ModelEventStream, ModelError> {
        let snapshot = self
            .token_source
            .get_token()
            .await
            .map_err(|error| ModelError::Transport(error.to_string()))?;
        let request_with_auth = self.prepare_request(request.clone(), &snapshot)?;
        match self
            .inner
            .send_event_stream_incremental(request_with_auth)
            .await
        {
            Err(ModelError::ProviderStatus { status: 401, .. }) => {
                let refreshed = self
                    .token_source
                    .refresh_token()
                    .await
                    .map_err(|error| ModelError::Transport(error.to_string()))?;
                self.inner
                    .send_event_stream_incremental(self.prepare_request(request, &refreshed)?)
                    .await
            }
            result => result,
        }
    }

    async fn send_websocket_event_stream_incremental(
        &self,
        request: HttpRequest,
    ) -> Result<ModelEventStream, ModelError> {
        let snapshot = self
            .token_source
            .get_token()
            .await
            .map_err(|error| ModelError::Transport(error.to_string()))?;
        let request_with_auth = self.prepare_request(request.clone(), &snapshot)?;
        match self
            .inner
            .send_websocket_event_stream_incremental(request_with_auth)
            .await
        {
            Err(ModelError::ProviderStatus { status: 401, .. }) => {
                let refreshed = self
                    .token_source
                    .refresh_token()
                    .await
                    .map_err(|error| ModelError::Transport(error.to_string()))?;
                self.inner
                    .send_websocket_event_stream_incremental(
                        self.prepare_request(request, &refreshed)?,
                    )
                    .await
            }
            result => result,
        }
    }
}

fn codex_user_agent() -> String {
    format!(
        "{}/{}",
        starweaver_core::sdk_name(),
        env!("CARGO_PKG_VERSION")
    )
}

const CODEX_SESSION_ROUTING_HEADER_NAMES: &[&str] = &["session_id", "session-id"];
const CODEX_THREAD_ROUTING_HEADER_NAMES: &[&str] =
    &["thread_id", "thread-id", "x-client-request-id"];
const CODEX_ROUTING_HEADER_NAMES: &[&str] = &[
    "session_id",
    "session-id",
    "thread_id",
    "thread-id",
    "x-client-request-id",
];

fn insert_header_if_absent_case_insensitive(
    headers: &mut BTreeMap<String, String>,
    name: &str,
    value: String,
) {
    if !headers.keys().any(|key| key.eq_ignore_ascii_case(name)) {
        headers.insert(name.to_string(), value);
    }
}

fn extract_case_insensitive_headers(
    headers: &BTreeMap<String, String>,
    names: &[&str],
) -> BTreeMap<String, String> {
    headers
        .iter()
        .filter(|(key, _)| names.iter().any(|name| key.eq_ignore_ascii_case(name)))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn restore_case_insensitive_headers(
    headers: &mut BTreeMap<String, String>,
    explicit_headers: BTreeMap<String, String>,
) {
    if explicit_headers
        .keys()
        .any(|key| key_matches_any(key, CODEX_SESSION_ROUTING_HEADER_NAMES))
    {
        remove_case_insensitive_headers(headers, CODEX_SESSION_ROUTING_HEADER_NAMES);
    }
    if explicit_headers
        .keys()
        .any(|key| key_matches_any(key, CODEX_THREAD_ROUTING_HEADER_NAMES))
    {
        remove_case_insensitive_headers(headers, CODEX_THREAD_ROUTING_HEADER_NAMES);
    }
    for (explicit_key, explicit_value) in explicit_headers {
        headers.retain(|key, _| !key.eq_ignore_ascii_case(&explicit_key));
        headers.insert(explicit_key, explicit_value);
    }
}

fn remove_case_insensitive_headers(headers: &mut BTreeMap<String, String>, names: &[&str]) {
    headers.retain(|key, _| !key_matches_any(key, names));
}

fn key_matches_any(key: &str, names: &[&str]) -> bool {
    names.iter().any(|name| key.eq_ignore_ascii_case(name))
}
