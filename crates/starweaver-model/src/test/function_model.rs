use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::{
    ModelAdapter, ModelError,
    adapter::{ModelRequestContext, ModelRequestParameters, ModelResponseEventStream},
    message::{ModelMessage, ModelResponse},
    profile::{ModelProfile, ProtocolFamily},
    settings::ModelSettings,
    stream::ModelResponseStreamEvent,
};

/// Information passed to a function-backed deterministic model.
#[derive(Clone, Debug)]
pub struct FunctionModelInfo {
    /// Model request parameters visible to this model call.
    pub params: ModelRequestParameters,
    /// Runtime request context.
    pub context: ModelRequestContext,
}

/// Function used by [`FunctionModel`] to produce responses.
pub type FunctionModelFn = dyn Send
    + Sync
    + Fn(
        Vec<ModelMessage>,
        Option<ModelSettings>,
        FunctionModelInfo,
    ) -> Result<ModelResponse, ModelError>;

/// Function used by [`FunctionModel`] to produce stream events.
pub type FunctionModelStreamFn = dyn Send
    + Sync
    + Fn(
        Vec<ModelMessage>,
        Option<ModelSettings>,
        FunctionModelInfo,
    ) -> Result<Vec<ModelResponseStreamEvent>, ModelError>;

/// Deterministic model backed by a caller-provided function.
#[derive(Clone)]
pub struct FunctionModel {
    model_name: String,
    profile: ModelProfile,
    default_settings: Option<ModelSettings>,
    function: Arc<FunctionModelFn>,
    stream_function: Arc<FunctionModelStreamFn>,
    captured_messages: Arc<Mutex<Vec<Vec<ModelMessage>>>>,
    captured_params: Arc<Mutex<Vec<ModelRequestParameters>>>,
}

impl FunctionModel {
    /// Create a function-backed model.
    #[must_use]
    pub fn new<F>(function: F) -> Self
    where
        F: Send
            + Sync
            + 'static
            + Fn(
                Vec<ModelMessage>,
                Option<ModelSettings>,
                FunctionModelInfo,
            ) -> Result<ModelResponse, ModelError>,
    {
        let function: Arc<FunctionModelFn> = Arc::new(function);
        let stream_function = function.clone();
        Self {
            model_name: "function".to_string(),
            profile: ModelProfile::for_protocol(ProtocolFamily::OpenAiChatCompletions),
            default_settings: None,
            function,
            stream_function: Arc::new(move |messages, settings, info| {
                stream_function(messages, settings, info)
                    .map(|response| vec![ModelResponseStreamEvent::FinalResult(Box::new(response))])
            }),
            captured_messages: Arc::new(Mutex::new(Vec::new())),
            captured_params: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Create a function-backed streaming model.
    #[must_use]
    pub fn streaming<F>(function: F) -> Self
    where
        F: Send
            + Sync
            + 'static
            + Fn(
                Vec<ModelMessage>,
                Option<ModelSettings>,
                FunctionModelInfo,
            ) -> Result<Vec<ModelResponseStreamEvent>, ModelError>,
    {
        Self {
            model_name: "function".to_string(),
            profile: ModelProfile::for_protocol(ProtocolFamily::OpenAiChatCompletions),
            default_settings: None,
            function: Arc::new(|_messages, _settings, _info| {
                Err(ModelError::Transport(
                    "function model response path is unavailable for streaming fixture".to_string(),
                ))
            }),
            stream_function: Arc::new(function),
            captured_messages: Arc::new(Mutex::new(Vec::new())),
            captured_params: Arc::new(Mutex::new(Vec::new())),
        }
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

#[async_trait]
impl ModelAdapter for FunctionModel {
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
        settings: Option<ModelSettings>,
        params: ModelRequestParameters,
        context: ModelRequestContext,
    ) -> Result<ModelResponse, ModelError> {
        if let Ok(mut captured) = self.captured_messages.lock() {
            captured.push(messages.clone());
        }
        if let Ok(mut captured) = self.captured_params.lock() {
            captured.push(params.clone());
        }
        (self.function)(messages, settings, FunctionModelInfo { params, context })
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
        settings: Option<ModelSettings>,
        params: ModelRequestParameters,
        context: ModelRequestContext,
    ) -> Result<ModelResponseEventStream, ModelError> {
        if let Ok(mut captured) = self.captured_messages.lock() {
            captured.push(messages.clone());
        }
        if let Ok(mut captured) = self.captured_params.lock() {
            captured.push(params.clone());
        }
        let events =
            (self.stream_function)(messages, settings, FunctionModelInfo { params, context })?;
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
