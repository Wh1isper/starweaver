//! Codex OAuth model adapter.

use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use serde_json::json;
use starweaver_oauth::OAuthTokenSource;

use crate::{
    oauth::OAuthBearerHttpClient,
    providers::client::ProtocolModelClient,
    transport::{DynHttpClient, HttpModelConfig},
    ModelAdapter, ModelError, ModelProfile, ModelRequestContext, ModelRequestParameters,
    ModelResponse, ModelResponseEventStream, ModelResponseStreamEvent, ModelRunSession,
    ModelSettings, ProtocolFamily,
};

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

    fn start_run_session(&self) -> Box<dyn ModelRunSession + '_> {
        self.inner.start_run_session()
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
