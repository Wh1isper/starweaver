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
        DynHttpClient, HttpRequest, HttpResponse, ModelEventStream, ModelHttpClient,
        ReqwestHttpClient,
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
            let mut extra_headers = self.extra_headers.clone();
            extra_headers.extend(trace_session_headers(&request));
            request.headers.extend(build_codex_headers(
                &snapshot.account,
                Some(&extra_headers),
            )?);
            patch_codex_responses_body(&mut request);
        } else {
            request.headers.extend(self.extra_headers.clone());
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
}

fn codex_user_agent() -> String {
    format!(
        "{}/{}",
        starweaver_core::sdk_name(),
        env!("CARGO_PKG_VERSION")
    )
}

fn insert_header_if_absent_case_insensitive(
    headers: &mut BTreeMap<String, String>,
    name: &str,
    value: String,
) {
    if !headers.keys().any(|key| key.eq_ignore_ascii_case(name)) {
        headers.insert(name.to_string(), value);
    }
}
