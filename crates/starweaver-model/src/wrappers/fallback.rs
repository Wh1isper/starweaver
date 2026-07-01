//! Fallback model wrapper.

use async_trait::async_trait;
use serde_json::{Value, json};

use super::DynModelAdapter;
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
