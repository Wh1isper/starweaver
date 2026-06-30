//! Model execution hook wrapper.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use starweaver_core::{ConversationId, RunId};
use starweaver_usage::Usage;

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

/// Shared model execution hook.
pub type DynModelExecutionHook = Arc<dyn ModelExecutionHook>;

/// Model wrapper metadata passed to execution hooks.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ModelExecutionMetadata {
    /// Provider model name.
    pub model_name: String,
    /// Provider name when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_name: Option<String>,
    /// Run id.
    pub run_id: RunId,
    /// Conversation id.
    pub conversation_id: ConversationId,
    /// Agent id when present in trace metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// Agent name when present in trace metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    /// Whether the request uses the streaming path.
    pub stream: bool,
    /// Low-cardinality context metadata copied from the runtime request context.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub context_metadata: Map<String, Value>,
}

impl ModelExecutionMetadata {
    fn new(model: &dyn ModelAdapter, context: &ModelRequestContext, stream: bool) -> Self {
        let agent_id = context
            .llm_trace_metadata
            .get("agent_id")
            .or_else(|| context.llm_trace_metadata.get("starweaver.agent_id"))
            .and_then(Value::as_str)
            .map(ToString::to_string);
        let agent_name = context
            .llm_trace_metadata
            .get("agent_name")
            .or_else(|| context.llm_trace_metadata.get("starweaver.agent_name"))
            .and_then(Value::as_str)
            .map(ToString::to_string);
        Self {
            model_name: model.model_name().to_string(),
            provider_name: model.provider_name().map(ToString::to_string),
            run_id: context.run_id.clone(),
            conversation_id: context.conversation_id.clone(),
            agent_id,
            agent_name,
            stream,
            context_metadata: context.llm_trace_metadata.clone(),
        }
    }
}

/// Hook around model adapter execution.
#[async_trait]
pub trait ModelExecutionHook: Send + Sync {
    /// Called before the wrapped model adapter receives a request.
    ///
    /// # Errors
    ///
    /// Returning an error prevents the inner model request from executing.
    async fn before_model_request(
        &self,
        _metadata: ModelExecutionMetadata,
        _messages: &[ModelMessage],
        _settings: Option<&ModelSettings>,
        _params: &ModelRequestParameters,
        _context: &ModelRequestContext,
    ) -> Result<(), ModelError> {
        Ok(())
    }

    /// Called after the wrapped model adapter returns a final response.
    ///
    /// # Errors
    ///
    /// Returning an error fails the model request.
    async fn after_model_response(
        &self,
        _metadata: ModelExecutionMetadata,
        _response: &ModelResponse,
    ) -> Result<(), ModelError> {
        Ok(())
    }

    /// Called when the wrapped model adapter returns an error.
    ///
    /// # Errors
    ///
    /// Returning an error replaces the original model error.
    async fn on_model_error(
        &self,
        _metadata: ModelExecutionMetadata,
        _error: &ModelError,
    ) -> Result<(), ModelError> {
        Ok(())
    }
}

/// Model wrapper that exposes request/response lifecycle hooks.
pub struct HookedModel {
    inner: DynModelAdapter,
    hooks: Vec<DynModelExecutionHook>,
}

struct HookedModelRunSession<'a> {
    model: &'a HookedModel,
    inner: Box<dyn ModelRunSession + 'a>,
}

impl HookedModel {
    /// Create a hooked model wrapper.
    #[must_use]
    pub fn new(inner: DynModelAdapter) -> Self {
        Self {
            inner,
            hooks: Vec::new(),
        }
    }

    /// Add one execution hook.
    #[must_use]
    pub fn with_hook(mut self, hook: DynModelExecutionHook) -> Self {
        self.hooks.push(hook);
        self
    }

    async fn call_before(
        &self,
        metadata: &ModelExecutionMetadata,
        messages: &[ModelMessage],
        settings: Option<&ModelSettings>,
        params: &ModelRequestParameters,
        context: &ModelRequestContext,
    ) -> Result<(), ModelError> {
        for hook in &self.hooks {
            hook.before_model_request(metadata.clone(), messages, settings, params, context)
                .await?;
        }
        Ok(())
    }

    async fn call_after(
        hooks: &[DynModelExecutionHook],
        metadata: &ModelExecutionMetadata,
        response: &ModelResponse,
    ) -> Result<(), ModelError> {
        for hook in hooks {
            hook.after_model_response(metadata.clone(), response)
                .await?;
        }
        Ok(())
    }

    async fn call_error(
        hooks: &[DynModelExecutionHook],
        metadata: &ModelExecutionMetadata,
        error: &ModelError,
    ) -> Result<(), ModelError> {
        for hook in hooks {
            hook.on_model_error(metadata.clone(), error).await?;
        }
        Ok(())
    }
}

#[async_trait]
impl ModelAdapter for HookedModel {
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
        Box::new(HookedModelRunSession {
            model: self,
            inner: self.inner.start_run_session(),
        })
    }

    async fn request(
        &self,
        messages: Vec<ModelMessage>,
        settings: Option<ModelSettings>,
        params: ModelRequestParameters,
        context: ModelRequestContext,
    ) -> Result<ModelResponse, ModelError> {
        let metadata = ModelExecutionMetadata::new(self.inner.as_ref(), &context, false);
        self.call_before(&metadata, &messages, settings.as_ref(), &params, &context)
            .await?;
        match self
            .inner
            .request(messages, settings, params, context)
            .await
        {
            Ok(response) => {
                Self::call_after(&self.hooks, &metadata, &response).await?;
                Ok(response)
            }
            Err(error) => {
                Self::call_error(&self.hooks, &metadata, &error).await?;
                Err(error)
            }
        }
    }

    async fn request_stream(
        &self,
        messages: Vec<ModelMessage>,
        settings: Option<ModelSettings>,
        params: ModelRequestParameters,
        context: ModelRequestContext,
    ) -> Result<Vec<ModelResponseStreamEvent>, ModelError> {
        let metadata = ModelExecutionMetadata::new(self.inner.as_ref(), &context, true);
        self.call_before(&metadata, &messages, settings.as_ref(), &params, &context)
            .await?;
        match self
            .inner
            .request_stream(messages, settings, params, context)
            .await
        {
            Ok(events) => {
                if let Some(response) = events.iter().find_map(|event| match event {
                    ModelResponseStreamEvent::FinalResult(response) => Some(response.as_ref()),
                    ModelResponseStreamEvent::PartStart(_)
                    | ModelResponseStreamEvent::PartDelta(_)
                    | ModelResponseStreamEvent::PartEnd(_)
                    | ModelResponseStreamEvent::Diagnostic(_) => None,
                }) {
                    Self::call_after(&self.hooks, &metadata, response).await?;
                }
                Ok(events)
            }
            Err(error) => {
                Self::call_error(&self.hooks, &metadata, &error).await?;
                Err(error)
            }
        }
    }

    async fn request_stream_incremental(
        &self,
        messages: Vec<ModelMessage>,
        settings: Option<ModelSettings>,
        params: ModelRequestParameters,
        context: ModelRequestContext,
    ) -> Result<ModelResponseEventStream, ModelError> {
        let metadata = ModelExecutionMetadata::new(self.inner.as_ref(), &context, true);
        self.call_before(&metadata, &messages, settings.as_ref(), &params, &context)
            .await?;
        match self
            .inner
            .request_stream_incremental(messages, settings, params, context)
            .await
        {
            Ok(mut inner_stream) => {
                let drop_abort_token = inner_stream.drop_abort_token();
                let hooks = self.hooks.clone();
                let (sender, receiver) = tokio::sync::mpsc::channel(32);
                tokio::spawn(async move {
                    while let Some(event) = inner_stream.recv().await {
                        match event {
                            Ok(ModelResponseStreamEvent::FinalResult(response)) => {
                                if let Err(error) =
                                    Self::call_after(&hooks, &metadata, &response).await
                                {
                                    let _ = sender.send(Err(error)).await;
                                    return;
                                }
                                if sender
                                    .send(Ok(ModelResponseStreamEvent::FinalResult(response)))
                                    .await
                                    .is_err()
                                {
                                    return;
                                }
                            }
                            Ok(event) => {
                                if sender.send(Ok(event)).await.is_err() {
                                    return;
                                }
                            }
                            Err(error) => {
                                let replacement =
                                    Self::call_error(&hooks, &metadata, &error).await.err();
                                let _ = sender.send(Err(replacement.unwrap_or(error))).await;
                                return;
                            }
                        }
                    }
                });
                Ok(
                    ModelResponseEventStream::new_with_cancellation_and_drop_abort(
                        receiver,
                        starweaver_core::CancellationToken::default(),
                        drop_abort_token,
                    ),
                )
            }
            Err(error) => {
                Self::call_error(&self.hooks, &metadata, &error).await?;
                Err(error)
            }
        }
    }

    async fn count_tokens(
        &self,
        messages: &[ModelMessage],
        settings: Option<&ModelSettings>,
        params: &ModelRequestParameters,
    ) -> Result<Usage, ModelError> {
        self.inner.count_tokens(messages, settings, params).await
    }
}

#[async_trait]
impl ModelRunSession for HookedModelRunSession<'_> {
    async fn request_stream_incremental(
        &mut self,
        messages: Vec<ModelMessage>,
        settings: Option<ModelSettings>,
        params: ModelRequestParameters,
        context: ModelRequestContext,
    ) -> Result<ModelResponseEventStream, ModelError> {
        let cancellation_token = context.cancellation_token();
        let metadata = ModelExecutionMetadata::new(self.model.inner.as_ref(), &context, true);
        self.model
            .call_before(&metadata, &messages, settings.as_ref(), &params, &context)
            .await?;
        match self
            .inner
            .request_stream_incremental(messages, settings, params, context)
            .await
        {
            Ok(mut inner_stream) => {
                let drop_abort_token = inner_stream.drop_abort_token();
                let hooks = self.model.hooks.clone();
                let (sender, receiver) = tokio::sync::mpsc::channel(32);
                tokio::spawn(async move {
                    while let Some(event) = inner_stream.recv().await {
                        match event {
                            Ok(ModelResponseStreamEvent::FinalResult(response)) => {
                                if let Err(error) =
                                    HookedModel::call_after(&hooks, &metadata, &response).await
                                {
                                    let _ = sender.send(Err(error)).await;
                                    return;
                                }
                                if sender
                                    .send(Ok(ModelResponseStreamEvent::FinalResult(response)))
                                    .await
                                    .is_err()
                                {
                                    return;
                                }
                            }
                            Ok(event) => {
                                if sender.send(Ok(event)).await.is_err() {
                                    return;
                                }
                            }
                            Err(error) => {
                                let replacement =
                                    HookedModel::call_error(&hooks, &metadata, &error)
                                        .await
                                        .err();
                                let _ = sender.send(Err(replacement.unwrap_or(error))).await;
                                return;
                            }
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
            Err(error) => {
                HookedModel::call_error(&self.model.hooks, &metadata, &error).await?;
                Err(error)
            }
        }
    }

    async fn close(&mut self) {
        self.inner.close().await;
    }
}
