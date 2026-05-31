//! Deterministic model adapters for agent tests.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::Value;
use starweaver_core::Usage;

use crate::{
    adapter::{ModelRequestContext, ModelRequestParameters},
    message::{
        ContentPart, ModelMessage, ModelRequestPart, ModelResponse, ModelResponsePart, ToolCallPart,
    },
    profile::{ModelProfile, ProtocolFamily},
    settings::ModelSettings,
    stream::ModelResponseStreamEvent,
    ModelAdapter, ModelError,
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
    pub const fn with_profile(mut self, profile: ModelProfile) -> Self {
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
        _settings: Option<ModelSettings>,
        params: ModelRequestParameters,
        _context: ModelRequestContext,
    ) -> Result<Vec<ModelResponseStreamEvent>, ModelError> {
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
        if let Some(events) = stream_events {
            return Ok(events);
        }
        let response = self
            .responses
            .lock()
            .map_err(|err| ModelError::Transport(err.to_string()))?
            .pop()
            .ok_or_else(|| ModelError::Transport("test model script exhausted".to_string()))?;
        Ok(vec![ModelResponseStreamEvent::FinalResult(Box::new(
            response,
        ))])
    }
}

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
    pub const fn with_profile(mut self, profile: ModelProfile) -> Self {
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
        if let Ok(mut captured) = self.captured_messages.lock() {
            captured.push(messages.clone());
        }
        if let Ok(mut captured) = self.captured_params.lock() {
            captured.push(params.clone());
        }
        (self.stream_function)(messages, settings, FunctionModelInfo { params, context })
    }
}

/// Create a tool call response for tests.
#[must_use]
pub fn tool_call_response(
    id: impl Into<String>,
    name: impl Into<String>,
    arguments: Value,
) -> ModelResponse {
    ModelResponse {
        parts: vec![ModelResponsePart::ToolCall(ToolCallPart {
            id: id.into(),
            name: name.into(),
            arguments,
        })],
        usage: Usage::default(),
        model_name: Some("test".to_string()),
        provider: None,
        finish_reason: None,
        timestamp: None,
        run_id: None,
        conversation_id: None,
        metadata: serde_json::Map::new(),
    }
}

/// Return the latest user prompt text from canonical history.
#[must_use]
pub fn latest_user_text(messages: &[ModelMessage]) -> Option<String> {
    messages.iter().rev().find_map(|message| match message {
        ModelMessage::Request(request) => request.parts.iter().rev().find_map(|part| match part {
            ModelRequestPart::UserPrompt { content, .. } => Some(text_from_content(content)),
            ModelRequestPart::RetryPrompt { text, .. }
            | ModelRequestPart::Instruction { text, .. }
            | ModelRequestPart::SystemPrompt { text, .. } => Some(text.clone()),
            ModelRequestPart::ToolReturn(_) => None,
        }),
        ModelMessage::Response(_) => None,
    })
}

fn text_from_content(content: &[ContentPart]) -> String {
    content
        .iter()
        .map(|part| match part {
            ContentPart::Text { text } => text.as_str(),
            ContentPart::ImageUrl { url } | ContentPart::FileUrl { url, .. } => url.as_str(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}
