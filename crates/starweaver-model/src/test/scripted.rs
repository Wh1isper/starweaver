use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::Value;

use crate::{
    adapter::{ModelRequestContext, ModelRequestParameters, ModelResponseEventStream},
    message::{ModelMessage, ModelResponse},
    profile::{ModelProfile, ProtocolFamily},
    settings::ModelSettings,
    stream::ModelResponseStreamEvent,
    ModelAdapter, ModelError,
};

/// Deterministic model that returns scripted responses.
#[derive(Clone)]
pub struct TestModel {
    model_name: String,
    profile: ModelProfile,
    default_settings: Option<ModelSettings>,
    responses: Arc<Mutex<Vec<ModelResponse>>>,
    stream_events: Arc<Mutex<Vec<Vec<ModelResponseStreamEvent>>>>,
    captured_messages: Arc<Mutex<Vec<Vec<ModelMessage>>>>,
    captured_params: Arc<Mutex<Vec<ModelRequestParameters>>>,
}

impl TestModel {
    /// Create a test model with a default text response.
    #[must_use]
    pub fn new() -> Self {
        Self::with_responses(vec![ModelResponse::text("ok")])
    }

    /// Create a test model with scripted responses in call order.
    #[must_use]
    pub fn with_responses(responses: Vec<ModelResponse>) -> Self {
        Self {
            model_name: "test".to_string(),
            profile: ModelProfile::for_protocol(ProtocolFamily::OpenAiChatCompletions),
            default_settings: None,
            responses: Arc::new(Mutex::new(responses.into_iter().rev().collect())),
            stream_events: Arc::new(Mutex::new(Vec::new())),
            captured_messages: Arc::new(Mutex::new(Vec::new())),
            captured_params: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Create a test model with scripted stream event batches in call order.
    #[must_use]
    pub fn with_stream_events(events: Vec<Vec<ModelResponseStreamEvent>>) -> Self {
        Self {
            model_name: "test".to_string(),
            profile: ModelProfile::for_protocol(ProtocolFamily::OpenAiChatCompletions),
            default_settings: None,
            responses: Arc::new(Mutex::new(Vec::new())),
            stream_events: Arc::new(Mutex::new(events.into_iter().rev().collect())),
            captured_messages: Arc::new(Mutex::new(Vec::new())),
            captured_params: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Create a test model returning plain text.
    #[must_use]
    pub fn with_text(text: impl Into<String>) -> Self {
        Self::with_responses(vec![ModelResponse::text(text)])
    }

    /// Create a test model returning JSON text.
    #[must_use]
    pub fn with_json(value: &Value) -> Self {
        Self::with_text(value.to_string())
    }

    /// Set model name.
    #[must_use]
    pub fn with_model_name(mut self, model_name: impl Into<String>) -> Self {
        self.model_name = model_name.into();
        self
    }

    /// Set model profile.
    #[must_use]
    pub fn with_profile(mut self, profile: ModelProfile) -> Self {
        self.profile = profile;
        self
    }

    /// Set model default settings.
    #[must_use]
    pub fn with_default_settings(mut self, settings: ModelSettings) -> Self {
        self.default_settings = Some(settings);
        self
    }

    /// Return captured request histories.
    #[must_use]
    pub fn captured_messages(&self) -> Vec<Vec<ModelMessage>> {
        self.captured_messages
            .lock()
            .map_or_else(|_| Vec::new(), |messages| messages.clone())
    }

    /// Return captured request parameters.
    #[must_use]
    pub fn captured_params(&self) -> Vec<ModelRequestParameters> {
        self.captured_params
            .lock()
            .map_or_else(|_| Vec::new(), |params| params.clone())
    }
}

impl Default for TestModel {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ModelAdapter for TestModel {
    fn model_name(&self) -> &str {
        &self.model_name
    }

    fn provider_name(&self) -> Option<&str> {
        Some("test")
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
        _settings: Option<ModelSettings>,
        params: ModelRequestParameters,
        _context: ModelRequestContext,
    ) -> Result<ModelResponse, ModelError> {
        if let Ok(mut captured) = self.captured_messages.lock() {
            captured.push(messages);
        }
        if let Ok(mut captured) = self.captured_params.lock() {
            captured.push(params);
        }
        self.responses
            .lock()
            .map_err(|err| ModelError::Transport(err.to_string()))?
            .pop()
            .ok_or_else(|| ModelError::Transport("test model script exhausted".to_string()))
    }

    async fn request_stream(
        &self,
        messages: Vec<ModelMessage>,
        settings: Option<ModelSettings>,
        params: ModelRequestParameters,
        context: ModelRequestContext,
    ) -> Result<Vec<ModelResponseStreamEvent>, ModelError> {
        let mut stream = self
            .request_stream_incremental(messages, settings, params, context)
            .await?;
        let mut events = Vec::new();
        while let Some(event) = stream.recv().await {
            events.push(event?);
        }
        Ok(events)
    }

    async fn request_stream_incremental(
        &self,
        messages: Vec<ModelMessage>,
        _settings: Option<ModelSettings>,
        params: ModelRequestParameters,
        _context: ModelRequestContext,
    ) -> Result<ModelResponseEventStream, ModelError> {
        if let Ok(mut captured) = self.captured_messages.lock() {
            captured.push(messages);
        }
        if let Ok(mut captured) = self.captured_params.lock() {
            captured.push(params);
        }
        let stream_events = self
            .stream_events
            .lock()
            .map_err(|err| ModelError::Transport(err.to_string()))?
            .pop();
        let events = if let Some(events) = stream_events {
            events
        } else {
            let response = self
                .responses
                .lock()
                .map_err(|err| ModelError::Transport(err.to_string()))?
                .pop()
                .ok_or_else(|| ModelError::Transport("test model script exhausted".to_string()))?;
            vec![ModelResponseStreamEvent::FinalResult(Box::new(response))]
        };
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
