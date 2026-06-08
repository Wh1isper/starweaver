//! Reusable model adapter wrappers.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::{
    adapter::{
        ModelAdapter, ModelError, ModelRequestContext, ModelRequestParameters,
        ModelResponseEventStream,
    },
    message::{ModelMessage, ModelResponse},
    profile::ModelProfile,
    settings::ModelSettings,
    stream::ModelResponseStreamEvent,
};

/// Shared model adapter reference used by wrappers.
pub type DynModelAdapter = Arc<dyn ModelAdapter>;

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
    ) -> Result<starweaver_core::Usage, ModelError> {
        self.inner.count_tokens(messages, settings, params).await
    }
}

/// Model wrapper that tries adapters in order until one succeeds.
pub struct FallbackModel {
    models: Vec<DynModelAdapter>,
    model_name: String,
    provider_name: Option<String>,
    profile: ModelProfile,
    default_settings: Option<ModelSettings>,
}

impl FallbackModel {
    /// Create a fallback wrapper from ordered candidate models.
    ///
    /// # Panics
    ///
    /// Panics when `models` is empty because a fallback wrapper requires at least one candidate.
    #[must_use]
    pub fn new(models: Vec<DynModelAdapter>) -> Self {
        assert!(
            !models.is_empty(),
            "fallback model requires at least one candidate"
        );
        let primary = models[0].clone();
        Self {
            model_name: primary.model_name().to_string(),
            provider_name: primary.provider_name().map(str::to_string),
            profile: primary.profile().clone(),
            default_settings: primary.default_settings().cloned(),
            models,
        }
    }

    /// Override the exposed model name.
    #[must_use]
    pub fn with_model_name(mut self, model_name: impl Into<String>) -> Self {
        self.model_name = model_name.into();
        self
    }

    /// Return candidate count.
    #[must_use]
    pub fn len(&self) -> usize {
        self.models.len()
    }

    /// Return whether there are no candidates.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }
}

#[async_trait]
impl ModelAdapter for FallbackModel {
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

    async fn request(
        &self,
        messages: Vec<ModelMessage>,
        settings: Option<ModelSettings>,
        params: ModelRequestParameters,
        context: ModelRequestContext,
    ) -> Result<ModelResponse, ModelError> {
        let mut failures = Vec::new();
        let mut attempts = 0u32;
        for model in &self.models {
            attempts += 1;
            let mut attempt_context = context.clone();
            annotate_attempt(&mut attempt_context, "request", attempts, model.as_ref());
            match model
                .request(
                    messages.clone(),
                    settings.clone(),
                    params.clone(),
                    attempt_context,
                )
                .await
            {
                Ok(mut response) => {
                    annotate_response_success(&mut response, attempts, model.as_ref(), &failures);
                    return Ok(response);
                }
                Err(error) => failures.push(fallback_failure(attempts, model.as_ref(), &error)),
            }
        }
        Err(fallback_error(attempts, failures))
    }

    async fn request_stream(
        &self,
        messages: Vec<ModelMessage>,
        settings: Option<ModelSettings>,
        params: ModelRequestParameters,
        context: ModelRequestContext,
    ) -> Result<Vec<ModelResponseStreamEvent>, ModelError> {
        let mut failures = Vec::new();
        let mut attempts = 0u32;
        for model in &self.models {
            attempts += 1;
            let mut attempt_context = context.clone();
            annotate_attempt(
                &mut attempt_context,
                "request_stream",
                attempts,
                model.as_ref(),
            );
            match model
                .request_stream(
                    messages.clone(),
                    settings.clone(),
                    params.clone(),
                    attempt_context,
                )
                .await
            {
                Ok(mut events) => {
                    annotate_stream_success(&mut events, attempts, model.as_ref(), &failures);
                    return Ok(events);
                }
                Err(error) => failures.push(fallback_failure(attempts, model.as_ref(), &error)),
            }
        }
        Err(fallback_error(attempts, failures))
    }

    async fn request_stream_incremental(
        &self,
        messages: Vec<ModelMessage>,
        settings: Option<ModelSettings>,
        params: ModelRequestParameters,
        context: ModelRequestContext,
    ) -> Result<ModelResponseEventStream, ModelError> {
        let events = self
            .request_stream(messages, settings, params, context)
            .await?;
        let (sender, receiver) = tokio::sync::mpsc::channel(events.len().max(1));
        tokio::spawn(async move {
            for event in events {
                if sender.send(Ok(event)).await.is_err() {
                    return;
                }
            }
        });
        Ok(ModelResponseEventStream::new(receiver))
    }
}

/// Model wrapper that limits concurrent calls with a shared semaphore.
pub struct ConcurrencyLimitedModel {
    inner: DynModelAdapter,
    semaphore: Arc<Semaphore>,
    max_concurrency: usize,
}

impl ConcurrencyLimitedModel {
    /// Create a concurrency-limited wrapper.
    #[must_use]
    pub fn new(inner: DynModelAdapter, max_concurrency: usize) -> Self {
        let permits = max_concurrency.max(1);
        Self {
            inner,
            semaphore: Arc::new(Semaphore::new(permits)),
            max_concurrency: permits,
        }
    }

    /// Create a wrapper using an existing shared semaphore.
    #[must_use]
    pub fn with_shared_semaphore(inner: DynModelAdapter, semaphore: Arc<Semaphore>) -> Self {
        Self {
            inner,
            max_concurrency: semaphore.available_permits().max(1),
            semaphore,
        }
    }

    /// Return configured max concurrency.
    #[must_use]
    pub const fn max_concurrency(&self) -> usize {
        self.max_concurrency
    }

    async fn acquire(&self) -> Result<OwnedSemaphorePermit, ModelError> {
        self.semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|error| {
                ModelError::Transport(format!("model concurrency limiter closed: {error}"))
            })
    }
}

#[async_trait]
impl ModelAdapter for ConcurrencyLimitedModel {
    fn model_name(&self) -> &str {
        self.inner.model_name()
    }

    fn provider_name(&self) -> Option<&str> {
        self.inner.provider_name()
    }

    fn profile(&self) -> &ModelProfile {
        self.inner.profile()
    }

    fn default_settings(&self) -> Option<&ModelSettings> {
        self.inner.default_settings()
    }

    async fn request(
        &self,
        messages: Vec<ModelMessage>,
        settings: Option<ModelSettings>,
        params: ModelRequestParameters,
        mut context: ModelRequestContext,
    ) -> Result<ModelResponse, ModelError> {
        let _permit = self.acquire().await?;
        annotate_limiter(&mut context, self.max_concurrency);
        self.inner
            .request(messages, settings, params, context)
            .await
    }

    async fn request_stream(
        &self,
        messages: Vec<ModelMessage>,
        settings: Option<ModelSettings>,
        params: ModelRequestParameters,
        mut context: ModelRequestContext,
    ) -> Result<Vec<ModelResponseStreamEvent>, ModelError> {
        let _permit = self.acquire().await?;
        annotate_limiter(&mut context, self.max_concurrency);
        self.inner
            .request_stream(messages, settings, params, context)
            .await
    }

    async fn request_stream_incremental(
        &self,
        messages: Vec<ModelMessage>,
        settings: Option<ModelSettings>,
        params: ModelRequestParameters,
        mut context: ModelRequestContext,
    ) -> Result<ModelResponseEventStream, ModelError> {
        let _permit = self.acquire().await?;
        annotate_limiter(&mut context, self.max_concurrency);
        let events = self
            .inner
            .request_stream(messages, settings, params, context)
            .await?;
        let (sender, receiver) = tokio::sync::mpsc::channel(events.len().max(1));
        tokio::spawn(async move {
            for event in events {
                if sender.send(Ok(event)).await.is_err() {
                    return;
                }
            }
            drop(_permit);
        });
        Ok(ModelResponseEventStream::new(receiver))
    }

    async fn count_tokens(
        &self,
        messages: &[ModelMessage],
        settings: Option<&ModelSettings>,
        params: &ModelRequestParameters,
    ) -> Result<starweaver_core::Usage, ModelError> {
        self.inner.count_tokens(messages, settings, params).await
    }
}

fn annotate_attempt(
    context: &mut ModelRequestContext,
    call_kind: &str,
    attempt: u32,
    model: &dyn ModelAdapter,
) {
    context.llm_trace_metadata.insert(
        "starweaver_model_wrapper".to_string(),
        json!({
            "kind": "fallback",
            "call_kind": call_kind,
            "attempt": attempt,
            "model": model.model_name(),
            "provider": model.provider_name(),
        }),
    );
}

fn annotate_limiter(context: &mut ModelRequestContext, max_concurrency: usize) {
    context.llm_trace_metadata.insert(
        "starweaver_model_wrapper".to_string(),
        json!({
            "kind": "concurrency_limited",
            "max_concurrency": max_concurrency,
        }),
    );
}

fn annotate_response_success(
    response: &mut ModelResponse,
    selected_attempt: u32,
    model: &dyn ModelAdapter,
    failures: &[Value],
) {
    response.metadata.insert(
        "starweaver_model_wrapper".to_string(),
        json!({
            "kind": "fallback",
            "selected_attempt": selected_attempt,
            "selected_model": model.model_name(),
            "selected_provider": model.provider_name(),
            "failures": failures,
        }),
    );
}

fn annotate_stream_success(
    events: &mut [ModelResponseStreamEvent],
    selected_attempt: u32,
    model: &dyn ModelAdapter,
    failures: &[Value],
) {
    for event in events.iter_mut() {
        if let ModelResponseStreamEvent::FinalResult(response) = event {
            annotate_response_success(response, selected_attempt, model, failures);
        }
    }
}

fn fallback_failure(attempt: u32, model: &dyn ModelAdapter, error: &ModelError) -> Value {
    json!({
        "attempt": attempt,
        "model": model.model_name(),
        "provider": model.provider_name(),
        "error": error.to_string(),
    })
}

fn fallback_error(attempts: u32, failures: Vec<Value>) -> ModelError {
    ModelError::Transport(format!(
        "fallback model exhausted after {attempts} attempts: {}",
        Value::Array(failures)
    ))
}
