//! Capability-profile override model wrapper.

use async_trait::async_trait;

use super::DynModelAdapter;
use crate::{
    adapter::{
        ModelAdapter, ModelError, ModelRequestContext, ModelRequestParameters,
        ModelResponseEventStream, ModelRunSession,
    },
    message::{ModelMessage, ModelResponse},
    profile::ModelProfile,
    settings::ModelSettings,
    stream::ModelResponseStreamEvent,
};

/// Model wrapper that overlays a capability profile and optional default settings.
pub struct ProfileOverrideModel {
    inner: DynModelAdapter,
    model_name: String,
    provider_name: Option<String>,
    profile: ModelProfile,
    default_settings: Option<ModelSettings>,
}

impl ProfileOverrideModel {
    /// Create a wrapper with a replacement profile.
    #[must_use]
    pub fn new(inner: DynModelAdapter, profile: ModelProfile) -> Self {
        Self {
            model_name: inner.model_name().to_string(),
            provider_name: inner.provider_name().map(str::to_string),
            default_settings: inner.default_settings().cloned(),
            inner,
            profile,
        }
    }

    /// Override the exposed model name.
    #[must_use]
    pub fn with_model_name(mut self, model_name: impl Into<String>) -> Self {
        self.model_name = model_name.into();
        self
    }

    /// Override the exposed provider name.
    #[must_use]
    pub fn with_provider_name(mut self, provider_name: impl Into<Option<String>>) -> Self {
        self.provider_name = provider_name.into();
        self
    }

    /// Override wrapper default settings.
    #[must_use]
    pub fn with_default_settings(mut self, settings: ModelSettings) -> Self {
        self.default_settings = Some(settings);
        self
    }
}

#[async_trait]
impl ModelAdapter for ProfileOverrideModel {
    fn model_name(&self) -> &str {
        &self.model_name
    }

    fn provider_name(&self) -> Option<&str> {
        self.provider_name.as_deref()
    }

    fn profile(&self) -> &ModelProfile {
        &self.profile
    }

    fn default_settings(&self) -> Option<&ModelSettings> {
        self.default_settings.as_ref()
    }

    fn start_run_session(&self) -> Box<dyn ModelRunSession + '_> {
        self.inner.start_run_session()
    }

    async fn request(
        &self,
        messages: Vec<ModelMessage>,
        settings: Option<ModelSettings>,
        params: ModelRequestParameters,
        context: ModelRequestContext,
    ) -> Result<ModelResponse, ModelError> {
        self.inner
            .request(messages, settings, params, context)
            .await
    }

    async fn request_stream(
        &self,
        messages: Vec<ModelMessage>,
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
        messages: Vec<ModelMessage>,
        settings: Option<ModelSettings>,
        params: ModelRequestParameters,
        context: ModelRequestContext,
    ) -> Result<ModelResponseEventStream, ModelError> {
        self.inner
            .request_stream_incremental(messages, settings, params, context)
            .await
    }

    async fn count_tokens(
        &self,
        messages: &[ModelMessage],
        settings: Option<&ModelSettings>,
        params: &ModelRequestParameters,
    ) -> Result<starweaver_usage::Usage, ModelError> {
        self.inner.count_tokens(messages, settings, params).await
    }
}
