use async_trait::async_trait;
use starweaver_usage::Usage;

use crate::{
    message::ModelMessage, profile::ModelProfile, settings::ModelSettings,
    stream::ModelResponseStreamEvent, ModelResponse,
};

use super::{ModelError, ModelRequestContext, ModelRequestParameters, ModelResponseEventStream};

/// Provider-neutral model adapter.
#[async_trait]
pub trait ModelAdapter: Send + Sync {
    /// Provider model name.
    fn model_name(&self) -> &str;

    /// Provider name.
    fn provider_name(&self) -> Option<&str>;

    /// Model capability profile.
    fn profile(&self) -> &ModelProfile;

    /// Default generation settings.
    fn default_settings(&self) -> Option<&ModelSettings>;

    /// Perform a complete model request.
    async fn request(
        &self,
        messages: Vec<ModelMessage>,
        settings: Option<ModelSettings>,
        params: ModelRequestParameters,
        context: ModelRequestContext,
    ) -> Result<ModelResponse, ModelError>;

    /// Stream a model request as canonical response part deltas.
    async fn request_stream(
        &self,
        messages: Vec<ModelMessage>,
        settings: Option<ModelSettings>,
        params: ModelRequestParameters,
        context: ModelRequestContext,
    ) -> Result<Vec<ModelResponseStreamEvent>, ModelError> {
        let response = self.request(messages, settings, params, context).await?;
        Ok(vec![ModelResponseStreamEvent::FinalResult(Box::new(
            response,
        ))])
    }

    /// Stream a model request and return the assembled final response.
    async fn request_stream_final(
        &self,
        messages: Vec<ModelMessage>,
        settings: Option<ModelSettings>,
        params: ModelRequestParameters,
        context: ModelRequestContext,
    ) -> Result<ModelResponse, ModelError> {
        let events = self
            .request_stream(messages, settings, params, context)
            .await?;
        events
            .into_iter()
            .find_map(|event| match event {
                ModelResponseStreamEvent::FinalResult(response) => Some(*response),
                ModelResponseStreamEvent::PartStart(_)
                | ModelResponseStreamEvent::PartDelta(_)
                | ModelResponseStreamEvent::PartEnd(_)
                | ModelResponseStreamEvent::Diagnostic(_) => None,
            })
            .ok_or_else(|| {
                ModelError::UnsupportedResponse(
                    "model stream did not produce a final result".to_string(),
                )
            })
    }

    /// Stream a model request and yield canonical events as they arrive.
    async fn request_stream_incremental(
        &self,
        messages: Vec<ModelMessage>,
        settings: Option<ModelSettings>,
        params: ModelRequestParameters,
        context: ModelRequestContext,
    ) -> Result<ModelResponseEventStream, ModelError> {
        let cancellation_token = context.cancellation_token();
        let events = self
            .request_stream(messages, settings, params, context)
            .await?;
        let (sender, receiver) = tokio::sync::mpsc::channel(events.len().max(1));
        for event in events {
            let _ = sender.send(Ok(event)).await;
        }
        Ok(ModelResponseEventStream::new_with_cancellation(
            receiver,
            cancellation_token,
        ))
    }

    /// Count tokens for a request where provider support exists.
    async fn count_tokens(
        &self,
        _messages: &[ModelMessage],
        _settings: Option<&ModelSettings>,
        _params: &ModelRequestParameters,
    ) -> Result<Usage, ModelError> {
        Ok(Usage::default())
    }
}
