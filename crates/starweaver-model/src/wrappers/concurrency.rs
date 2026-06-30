//! Concurrency-limited model wrapper.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

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

/// Model wrapper that limits concurrent calls with a shared semaphore.
pub struct ConcurrencyLimitedModel {
    inner: DynModelAdapter,
    semaphore: Arc<Semaphore>,
    max_concurrency: usize,
}

struct ConcurrencyLimitedRunSession<'a> {
    model: &'a ConcurrencyLimitedModel,
    inner: Box<dyn ModelRunSession + 'a>,
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

    fn start_run_session(&self) -> Box<dyn ModelRunSession + '_> {
        Box::new(ConcurrencyLimitedRunSession {
            model: self,
            inner: self.inner.start_run_session(),
        })
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
        let permit = self.acquire().await?;
        annotate_limiter(&mut context, self.max_concurrency);
        let events = self
            .inner
            .request_stream(messages, settings, params, context)
            .await?;
        let (sender, receiver) = tokio::sync::mpsc::channel(events.len().max(1));
        tokio::spawn(async move {
            let _permit = permit;
            for event in events {
                if sender.send(Ok(event)).await.is_err() {
                    return;
                }
            }
        });
        Ok(ModelResponseEventStream::new(receiver))
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

#[async_trait]
impl ModelRunSession for ConcurrencyLimitedRunSession<'_> {
    async fn request_stream_incremental(
        &mut self,
        messages: Vec<ModelMessage>,
        settings: Option<ModelSettings>,
        params: ModelRequestParameters,
        mut context: ModelRequestContext,
    ) -> Result<ModelResponseEventStream, ModelError> {
        let cancellation_token = context.cancellation_token();
        let permit = self.model.acquire().await?;
        annotate_limiter(&mut context, self.model.max_concurrency);
        let mut events = self
            .inner
            .request_stream_incremental(messages, settings, params, context)
            .await?;
        let drop_abort_token = events.drop_abort_token();
        let (sender, receiver) = tokio::sync::mpsc::channel(32);
        tokio::spawn(async move {
            let _permit = permit;
            while let Some(event) = events.recv().await {
                if sender.send(event).await.is_err() {
                    return;
                }
            }
        });
        Ok(
            ModelResponseEventStream::new_with_cancellation_and_drop_abort(
                receiver,
                cancellation_token,
                drop_abort_token,
            ),
        )
    }

    async fn request_stream_final(
        &mut self,
        messages: Vec<ModelMessage>,
        settings: Option<ModelSettings>,
        params: ModelRequestParameters,
        mut context: ModelRequestContext,
    ) -> Result<ModelResponse, ModelError> {
        let _permit = self.model.acquire().await?;
        annotate_limiter(&mut context, self.model.max_concurrency);
        self.inner
            .request_stream_final(messages, settings, params, context)
            .await
    }

    async fn close(&mut self) {
        self.inner.close().await;
    }
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
