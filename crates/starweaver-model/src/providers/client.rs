//! Production protocol clients built on replay-validated wire mappers.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::{
    adapter::{allow_real_model_requests, ModelRequestContext, ModelRequestParameters},
    message::{ModelMessage, ModelResponse},
    profile::{ModelProfile, ProtocolFamily},
    providers::{
        anthropic::AnthropicMessagesAdapter, bedrock::BedrockConverseAdapter,
        gemini::GeminiGenerateContentAdapter, openai_chat::OpenAiChatAdapter,
        openai_responses::OpenAiResponsesAdapter,
    },
    settings::ModelSettings,
    transport::{
        build_http_request, send_with_retries, DynHttpClient, DynSleeper, HttpModelConfig,
        HttpRequestOptions, MaxTokensParameter, ReqwestHttpClient, TokioSleeper,
    },
    ModelAdapter, ModelError, ModelResponseEventStream, ModelResponseStreamEvent,
};

/// Shared production model client for a supported wire protocol family.
pub struct ProtocolModelClient {
    provider_name: String,
    model_name: String,
    profile: ModelProfile,
    default_settings: Option<ModelSettings>,
    http_config: HttpModelConfig,
    http_client: DynHttpClient,
    sleeper: DynSleeper,
}

impl ProtocolModelClient {
    /// Create a protocol client with an injected HTTP client.
    #[must_use]
    pub fn new(
        provider_name: impl Into<String>,
        model_name: impl Into<String>,
        profile: ModelProfile,
        http_config: HttpModelConfig,
        http_client: DynHttpClient,
    ) -> Self {
        Self {
            provider_name: provider_name.into(),
            model_name: model_name.into(),
            profile,
            default_settings: None,
            http_config,
            http_client,
            sleeper: std::sync::Arc::new(TokioSleeper),
        }
    }

    /// Set adapter-level default settings.
    #[must_use]
    pub fn with_default_settings(mut self, settings: ModelSettings) -> Self {
        self.default_settings = Some(settings);
        self
    }

    /// Override the model capability profile.
    #[must_use]
    pub const fn with_profile(mut self, profile: ModelProfile) -> Self {
        self.profile = profile;
        self
    }

    /// Set a custom sleeper for retry policy execution.
    #[must_use]
    pub fn with_sleeper(mut self, sleeper: DynSleeper) -> Self {
        self.sleeper = sleeper;
        self
    }

    /// Create an `OpenAI` Chat Completions client.
    ///
    /// # Errors
    ///
    /// Returns an error when the default HTTP client cannot be created.
    pub fn openai_chat(
        model_name: impl Into<String>,
        token: impl Into<String>,
    ) -> Result<Self, ModelError> {
        let mut config = HttpModelConfig::new("https://api.openai.com/v1", "chat/completions");
        config.auth = Some(crate::transport::AuthConfig::Bearer {
            token: token.into(),
        });
        Ok(Self::new(
            "openai",
            model_name,
            ModelProfile::for_protocol(ProtocolFamily::OpenAiChatCompletions),
            config,
            std::sync::Arc::new(ReqwestHttpClient::new()?),
        ))
    }

    /// Create an `OpenAI` Responses client.
    ///
    /// # Errors
    ///
    /// Returns an error when the default HTTP client cannot be created.
    pub fn openai_responses(
        model_name: impl Into<String>,
        token: impl Into<String>,
    ) -> Result<Self, ModelError> {
        let mut config = HttpModelConfig::new("https://api.openai.com/v1", "responses");
        config.auth = Some(crate::transport::AuthConfig::Bearer {
            token: token.into(),
        });
        Ok(Self::new(
            "openai",
            model_name,
            ModelProfile::for_protocol(ProtocolFamily::OpenAiResponses),
            config,
            std::sync::Arc::new(ReqwestHttpClient::new()?),
        ))
    }

    /// Create an Anthropic Messages client.
    ///
    /// # Errors
    ///
    /// Returns an error when the default HTTP client cannot be created.
    pub fn anthropic_messages(
        model_name: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Result<Self, ModelError> {
        let mut config = HttpModelConfig::new("https://api.anthropic.com/v1", "messages");
        config.auth = Some(crate::transport::AuthConfig::Header {
            name: "x-api-key".to_string(),
            value: api_key.into(),
        });
        config
            .headers
            .insert("anthropic-version".to_string(), "2023-06-01".to_string());
        Ok(Self::new(
            "anthropic",
            model_name,
            ModelProfile::for_protocol(ProtocolFamily::AnthropicMessages),
            config,
            std::sync::Arc::new(ReqwestHttpClient::new()?),
        ))
    }

    /// Create a Gemini generateContent client.
    ///
    /// # Errors
    ///
    /// Returns an error when the default HTTP client cannot be created.
    pub fn gemini_generate_content(
        model_name: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Result<Self, ModelError> {
        let model_name = model_name.into();
        let config = HttpModelConfig::new(
            "https://generativelanguage.googleapis.com/v1beta",
            format!("models/{model_name}:generateContent?key={}", api_key.into()),
        );
        Ok(Self::new(
            "gemini",
            model_name,
            ModelProfile::for_protocol(ProtocolFamily::GeminiGenerateContent),
            config,
            std::sync::Arc::new(ReqwestHttpClient::new()?),
        ))
    }

    /// Create a Bedrock Converse client for a gateway-compatible endpoint.
    ///
    /// Bedrock production calls usually require AWS `SigV4`. Gateways and signed clients can inject
    /// the final endpoint and headers through `HttpModelConfig` and `ModelHttpClient`.
    #[must_use]
    pub fn bedrock_converse_gateway(
        model_name: impl Into<String>,
        http_config: HttpModelConfig,
        http_client: DynHttpClient,
    ) -> Self {
        Self::new(
            "bedrock",
            model_name,
            ModelProfile::for_protocol(ProtocolFamily::BedrockConverse),
            http_config,
            http_client,
        )
    }

    fn merged_settings(&self, settings: Option<ModelSettings>) -> Option<ModelSettings> {
        match (&self.default_settings, settings) {
            (Some(defaults), Some(settings)) => Some(defaults.merge(&settings)),
            (Some(defaults), None) => Some(defaults.clone()),
            (None, Some(settings)) => Some(settings),
            (None, None) => None,
        }
    }

    fn build_wire_body(
        &self,
        messages: &[ModelMessage],
        settings: Option<&ModelSettings>,
        params: &ModelRequestParameters,
    ) -> Result<Value, ModelError> {
        let mut body = match self.profile.protocol {
            ProtocolFamily::OpenAiChatCompletions => OpenAiChatAdapter::build_request(
                &self.model_name,
                messages,
                settings,
                &params.tools,
            ),
            ProtocolFamily::OpenAiResponses => OpenAiResponsesAdapter::build_request_with_options(
                &self.model_name,
                messages,
                settings,
                &params.tools,
                &params.native_tools,
                self.openai_responses_max_tokens_parameter(),
            ),
            ProtocolFamily::AnthropicMessages => AnthropicMessagesAdapter::build_request(
                &self.model_name,
                messages,
                settings,
                &params.tools,
            ),
            ProtocolFamily::GeminiGenerateContent => {
                GeminiGenerateContentAdapter::build_request_with_native_tools(
                    messages,
                    settings,
                    &params.tools,
                    &params.native_tools,
                )
            }
            ProtocolFamily::BedrockConverse => BedrockConverseAdapter::build_request(
                &self.model_name,
                messages,
                settings,
                &params.tools,
            ),
        }?;
        apply_output_schema(&mut body, &self.profile, params.output_schema.as_ref());
        Ok(body)
    }

    const fn openai_responses_max_tokens_parameter(&self) -> MaxTokensParameter {
        match self.http_config.max_tokens_parameter {
            MaxTokensParameter::Default => MaxTokensParameter::MaxTokens,
            value => value,
        }
    }

    fn parse_wire_response(&self, body: &Value) -> Result<ModelResponse, ModelError> {
        match self.profile.protocol {
            ProtocolFamily::OpenAiChatCompletions => OpenAiChatAdapter::parse_response(body),
            ProtocolFamily::OpenAiResponses => OpenAiResponsesAdapter::parse_response(body),
            ProtocolFamily::AnthropicMessages => AnthropicMessagesAdapter::parse_response(body),
            ProtocolFamily::GeminiGenerateContent => {
                GeminiGenerateContentAdapter::parse_response(body)
            }
            ProtocolFamily::BedrockConverse => BedrockConverseAdapter::parse_response(body),
        }
    }

    fn request_options(
        context: &ModelRequestContext,
        settings: Option<&ModelSettings>,
        params: &ModelRequestParameters,
    ) -> HttpRequestOptions {
        let mut options = params.http.clone();
        if let Some(settings) = settings {
            options.headers.extend(settings.extra_headers.clone());
            options.extra_body.extend(settings.extra_body.clone());
        }
        options.extra_body.extend(params.extra_body.clone());
        options.metadata.extend(context.llm_trace_metadata.clone());
        options.metadata.insert(
            "starweaver.run_id".to_string(),
            json!(context.run_id.as_str()),
        );
        options.metadata.insert(
            "starweaver.conversation_id".to_string(),
            json!(context.conversation_id.as_str()),
        );
        options
    }
}

fn apply_output_schema(body: &mut Value, profile: &ModelProfile, schema: Option<&Value>) {
    let Some(schema) = schema else {
        return;
    };
    let Some(object) = body.as_object_mut() else {
        return;
    };
    match profile.protocol {
        ProtocolFamily::OpenAiChatCompletions => {
            object.insert(
                "response_format".to_string(),
                serde_json::json!({
                    "type": "json_schema",
                    "json_schema": schema,
                }),
            );
        }
        ProtocolFamily::OpenAiResponses => {
            object.insert(
                "text".to_string(),
                serde_json::json!({
                    "format": {
                        "type": "json_schema",
                        "name": schema.get("name").and_then(Value::as_str).unwrap_or("output"),
                        "schema": schema.get("schema").cloned().unwrap_or_else(|| schema.clone()),
                        "strict": schema.get("strict").and_then(Value::as_bool).unwrap_or(true),
                    }
                }),
            );
        }
        ProtocolFamily::GeminiGenerateContent => {
            let generation_config = object
                .entry("generationConfig".to_string())
                .or_insert_with(|| Value::Object(serde_json::Map::new()));
            if let Some(generation_config) = generation_config.as_object_mut() {
                generation_config.insert(
                    "responseMimeType".to_string(),
                    Value::String("application/json".to_string()),
                );
                generation_config.insert(
                    "responseSchema".to_string(),
                    schema
                        .get("schema")
                        .cloned()
                        .unwrap_or_else(|| schema.clone()),
                );
            }
        }
        ProtocolFamily::AnthropicMessages | ProtocolFamily::BedrockConverse => {}
    }
}

#[async_trait]
impl ModelAdapter for ProtocolModelClient {
    fn model_name(&self) -> &str {
        &self.model_name
    }

    fn provider_name(&self) -> Option<&str> {
        Some(&self.provider_name)
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
        let settings = self.merged_settings(settings);
        let wire_body = self.build_wire_body(&messages, settings.as_ref(), &params)?;
        let options = Self::request_options(&context, settings.as_ref(), &params);
        let request = build_http_request(&self.http_config, &options, wire_body);
        if !allow_real_model_requests() {
            return Err(ModelError::RealModelRequestBlocked { url: request.url });
        }
        let response = send_with_retries(
            self.http_client.as_ref(),
            self.sleeper.as_ref(),
            request,
            &self.http_config.retry_policy,
        )
        .await?;
        self.parse_wire_response(&response.body)
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
        if self.profile.protocol != ProtocolFamily::OpenAiResponses {
            let response = self.request(messages, settings, params, context).await?;
            let (sender, receiver) = tokio::sync::mpsc::channel(1);
            let _ = sender
                .send(Ok(ModelResponseStreamEvent::FinalResult(Box::new(
                    response,
                ))))
                .await;
            return Ok(ModelResponseEventStream::new(receiver));
        }
        let settings = self.merged_settings(settings);
        let mut wire_body = self.build_wire_body(&messages, settings.as_ref(), &params)?;
        if let Some(object) = wire_body.as_object_mut() {
            object.insert("stream".to_string(), Value::Bool(true));
        }
        let options = Self::request_options(&context, settings.as_ref(), &params);
        let request = build_http_request(&self.http_config, &options, wire_body);
        if !allow_real_model_requests() {
            return Err(ModelError::RealModelRequestBlocked { url: request.url });
        }
        let mut events = self
            .http_client
            .send_event_stream_incremental(request)
            .await?;
        let (sender, receiver) = tokio::sync::mpsc::channel(32);
        tokio::spawn(async move {
            let mut parser =
                crate::providers::openai_responses::OpenAiResponsesStreamParser::default();
            while let Some(event) = events.recv().await {
                let events = match event.and_then(|event| parser.push_event(&event)) {
                    Ok(events) => events,
                    Err(error) => {
                        let _ = sender.send(Err(error)).await;
                        return;
                    }
                };
                for event in events {
                    if sender.send(Ok(event)).await.is_err() {
                        return;
                    }
                }
            }
            match parser.finish() {
                Ok(events) => {
                    for event in events {
                        if sender.send(Ok(event)).await.is_err() {
                            return;
                        }
                    }
                }
                Err(error) => {
                    let _ = sender.send(Err(error)).await;
                }
            }
        });
        Ok(ModelResponseEventStream::new(receiver))
    }
}
