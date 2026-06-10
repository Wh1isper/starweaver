//! OAuth-backed model provider integration.

use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use serde_json::{json, Value};
use starweaver_oauth::{OAuthAccount, OAuthTokenSource};

use crate::{
    providers::client::ProtocolModelClient,
    transport::{
        DynHttpClient, HttpMethod, HttpModelConfig, HttpRequest, HttpResponse, ModelEventStream,
        ModelHttpClient, ReqwestHttpClient,
    },
    ModelAdapter, ModelError, ModelProfile, ModelRequestContext, ModelRequestParameters,
    ModelResponse, ModelResponseEventStream, ModelResponseStreamEvent, ModelSettings,
    ProtocolFamily,
};

/// Codex request header originator used by Starweaver OAuth-backed requests.
pub const CODEX_ORIGINATOR: &str = "starweaver";

const CODEX_USER_AGENT_HEADER: &str = "User-Agent";

/// Reserved headers that user-provided OAuth extra headers may not override.
pub const RESERVED_OAUTH_EXTRA_HEADERS: &[&str] = &[
    "authorization",
    "proxy-authorization",
    "chatgpt-account-id",
    "x-openai-fedramp",
    "originator",
    "version",
];

/// Build Codex-compatible request headers without an Authorization header.
///
/// # Errors
///
/// Returns an error when `extra_headers` attempts to override an OAuth/Codex reserved header.
pub fn build_codex_headers(
    account: &OAuthAccount,
    extra_headers: Option<&BTreeMap<String, String>>,
) -> Result<BTreeMap<String, String>, ModelError> {
    let mut headers = BTreeMap::from([("originator".to_string(), CODEX_ORIGINATOR.to_string())]);
    if let Some(account_id) = account.chatgpt_account_id.as_ref() {
        headers.insert("ChatGPT-Account-ID".to_string(), account_id.clone());
    }
    if account.chatgpt_account_is_fedramp {
        headers.insert("X-OpenAI-Fedramp".to_string(), "true".to_string());
    }
    for (key, value) in extra_headers.unwrap_or(&BTreeMap::new()) {
        if RESERVED_OAUTH_EXTRA_HEADERS
            .iter()
            .any(|reserved| key.eq_ignore_ascii_case(reserved))
        {
            return Err(ModelError::Transport(format!(
                "extra_headers may not override reserved OAuth/Codex header: {key}"
            )));
        }
        headers.insert(key.clone(), value.clone());
    }
    Ok(headers)
}

/// Build Codex session/thread headers with underscore and hyphen variants.
#[must_use]
pub fn build_session_headers(
    session_id: Option<&str>,
    thread_id: Option<&str>,
) -> BTreeMap<String, String> {
    let mut headers = BTreeMap::new();
    if let Some(session_id) = session_id.filter(|value| !value.is_empty()) {
        headers.insert("session_id".to_string(), session_id.to_string());
        headers.insert("session-id".to_string(), session_id.to_string());
    }
    if let Some(thread_id) = thread_id.filter(|value| !value.is_empty()) {
        headers.insert("thread_id".to_string(), thread_id.to_string());
        headers.insert("thread-id".to_string(), thread_id.to_string());
        headers.insert("x-client-request-id".to_string(), thread_id.to_string());
    }
    headers
}

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

fn validate_safe_extra_headers(extra_headers: &BTreeMap<String, String>) -> Result<(), ModelError> {
    for key in extra_headers.keys() {
        if RESERVED_OAUTH_EXTRA_HEADERS
            .iter()
            .any(|reserved| key.eq_ignore_ascii_case(reserved))
        {
            return Err(ModelError::Transport(format!(
                "extra_headers may not override reserved OAuth/Codex header: {key}"
            )));
        }
    }
    Ok(())
}

fn trace_session_headers(request: &HttpRequest) -> BTreeMap<String, String> {
    let session_id = metadata_string(request, "starweaver.conversation_id");
    let thread_id = metadata_string(request, "starweaver.run_id")
        .or_else(|| metadata_string(request, "starweaver.conversation_id"));
    build_session_headers(session_id.as_deref(), thread_id.as_deref())
}

fn metadata_string(request: &HttpRequest, key: &str) -> Option<String> {
    request
        .metadata
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

/// Align Codex Responses API body requirements.
pub fn patch_codex_responses_body(request: &mut HttpRequest) {
    if request.method != HttpMethod::Post || !is_codex_responses_path(&request.url) {
        return;
    }
    let Some(body) = request.body.as_object_mut() else {
        return;
    };
    if body
        .get("instructions")
        .map_or(true, codex_instructions_value_is_falsy)
    {
        body.insert("instructions".to_string(), Value::String(String::new()));
    }
    body.insert("store".to_string(), Value::Bool(false));
}

fn is_codex_responses_path(url: &str) -> bool {
    reqwest::Url::parse(url)
        .is_ok_and(|url| url.path().trim_end_matches('/') == "/backend-api/codex/responses")
}

fn codex_instructions_value_is_falsy(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Bool(value) => !value,
        Value::Number(value) => {
            value.as_i64().is_some_and(|value| value == 0)
                || value.as_u64().is_some_and(|value| value == 0)
                || value.as_f64().is_some_and(|value| value == 0.0)
        }
        Value::String(value) => value.is_empty(),
        Value::Array(value) => value.is_empty(),
        Value::Object(value) => value.is_empty(),
    }
}

/// `Codex` OAuth-backed `OpenAI` Responses model.
pub struct CodexOAuthResponsesModel {
    inner: ProtocolModelClient,
}

impl CodexOAuthResponsesModel {
    /// Create a Codex OAuth-backed `OpenAI` Responses model.
    ///
    /// # Errors
    ///
    /// Returns an error when the OAuth HTTP client cannot be built.
    pub fn new(
        model_name: impl Into<String>,
        http_config: HttpModelConfig,
        token_source: Arc<dyn OAuthTokenSource>,
        extra_headers: BTreeMap<String, String>,
    ) -> Result<Self, ModelError> {
        Self::new_with_profile(
            model_name,
            http_config,
            token_source,
            extra_headers,
            codex_model_profile(),
        )
    }

    /// Create a Codex OAuth-backed model with an explicit capability profile.
    ///
    /// # Errors
    ///
    /// Returns an error when the OAuth HTTP client cannot be built.
    pub fn new_with_profile(
        model_name: impl Into<String>,
        http_config: HttpModelConfig,
        token_source: Arc<dyn OAuthTokenSource>,
        extra_headers: BTreeMap<String, String>,
        profile: ModelProfile,
    ) -> Result<Self, ModelError> {
        let client =
            OAuthBearerHttpClient::with_default_http_client(token_source, "codex", extra_headers)?;
        Ok(Self {
            inner: ProtocolModelClient::new(
                "codex",
                model_name,
                profile,
                http_config,
                Arc::new(client),
            ),
        })
    }

    /// Create a model with an injected HTTP client.
    ///
    /// # Errors
    ///
    /// Returns an error when `extra_headers` contains reserved headers.
    pub fn with_http_client(
        model_name: impl Into<String>,
        http_config: HttpModelConfig,
        token_source: Arc<dyn OAuthTokenSource>,
        extra_headers: BTreeMap<String, String>,
        inner_http_client: DynHttpClient,
    ) -> Result<Self, ModelError> {
        Self::with_http_client_and_profile(
            model_name,
            http_config,
            token_source,
            extra_headers,
            inner_http_client,
            codex_model_profile(),
        )
    }

    /// Create a model with an injected HTTP client and explicit capability profile.
    ///
    /// # Errors
    ///
    /// Returns an error when `extra_headers` contains reserved headers.
    pub fn with_http_client_and_profile(
        model_name: impl Into<String>,
        http_config: HttpModelConfig,
        token_source: Arc<dyn OAuthTokenSource>,
        extra_headers: BTreeMap<String, String>,
        inner_http_client: DynHttpClient,
        profile: ModelProfile,
    ) -> Result<Self, ModelError> {
        let client =
            OAuthBearerHttpClient::new(inner_http_client, token_source, "codex", extra_headers)?;
        Ok(Self {
            inner: ProtocolModelClient::new(
                "codex",
                model_name,
                profile,
                http_config,
                Arc::new(client),
            ),
        })
    }
}

#[async_trait]
impl ModelAdapter for CodexOAuthResponsesModel {
    fn model_name(&self) -> &str {
        self.inner.model_name()
    }

    fn provider_name(&self) -> Option<&str> {
        Some("codex")
    }

    fn profile(&self) -> &ModelProfile {
        self.inner.profile()
    }

    fn default_settings(&self) -> Option<&ModelSettings> {
        self.inner.default_settings()
    }

    async fn request(
        &self,
        _messages: Vec<crate::ModelMessage>,
        _settings: Option<ModelSettings>,
        _params: ModelRequestParameters,
        _context: ModelRequestContext,
    ) -> Result<ModelResponse, ModelError> {
        Err(ModelError::Transport(
            "Codex OAuth Responses API requires streaming. Use request_stream_incremental or a streaming agent run."
                .to_string(),
        ))
    }

    async fn request_stream(
        &self,
        messages: Vec<crate::ModelMessage>,
        settings: Option<ModelSettings>,
        params: ModelRequestParameters,
        context: ModelRequestContext,
    ) -> Result<Vec<ModelResponseStreamEvent>, ModelError> {
        self.inner
            .request_stream(messages, settings, params, context)
            .await
    }

    async fn request_stream_incremental(
        &self,
        messages: Vec<crate::ModelMessage>,
        settings: Option<ModelSettings>,
        params: ModelRequestParameters,
        context: ModelRequestContext,
    ) -> Result<ModelResponseEventStream, ModelError> {
        self.inner
            .request_stream_incremental(messages, settings, params, context)
            .await
    }
}

/// Build the Codex model capability profile.
#[must_use]
pub fn codex_model_profile() -> ModelProfile {
    let mut profile = ModelProfile::for_protocol(ProtocolFamily::OpenAiResponses);
    profile.supports_tools = true;
    profile.supports_json_schema_output = true;
    profile.supports_thinking = true;
    profile.thinking_always_enabled = true;
    profile
}

/// Build a Codex OAuth model from model name and token source.
///
/// # Errors
///
/// Returns an error when OAuth HTTP client construction fails.
pub fn build_codex_model(
    model_name: impl Into<String>,
    token_source: Arc<dyn OAuthTokenSource>,
    mut http_config: HttpModelConfig,
    extra_headers: BTreeMap<String, String>,
) -> Result<CodexOAuthResponsesModel, ModelError> {
    http_config
        .metadata
        .insert("oauth_provider".to_string(), json!("codex"));
    CodexOAuthResponsesModel::new(model_name, http_config, token_source, extra_headers)
}

/// Build a Codex OAuth model with an explicit capability profile.
///
/// # Errors
///
/// Returns an error when OAuth HTTP client construction fails.
pub fn build_codex_model_with_profile(
    model_name: impl Into<String>,
    token_source: Arc<dyn OAuthTokenSource>,
    mut http_config: HttpModelConfig,
    extra_headers: BTreeMap<String, String>,
    profile: ModelProfile,
) -> Result<CodexOAuthResponsesModel, ModelError> {
    http_config
        .metadata
        .insert("oauth_provider".to_string(), json!("codex"));
    CodexOAuthResponsesModel::new_with_profile(
        model_name,
        http_config,
        token_source,
        extra_headers,
        profile,
    )
}
