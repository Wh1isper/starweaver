use std::sync::Arc;

use crate::{
    ModelError,
    presets::{
        anthropic_http_config, gemini_http_config, openai_chat_http_config,
        openai_responses_http_config,
    },
    profile::{ModelProfile, ProtocolFamily},
    transport::{DynHttpClient, HttpModelConfig, ReqwestHttpClient},
};

use super::ProtocolModelClient;

impl ProtocolModelClient {
    /// Create an `OpenAI` Chat Completions client.
    ///
    /// # Errors
    ///
    /// Returns an error when the default HTTP client cannot be created.
    pub fn openai_chat(
        model_name: impl Into<String>,
        token: impl Into<String>,
    ) -> Result<Self, ModelError> {
        let config = openai_chat_http_config(token);
        Ok(Self::new(
            "openai",
            model_name,
            ModelProfile::for_protocol(ProtocolFamily::OpenAiChatCompletions),
            config,
            Arc::new(ReqwestHttpClient::new()?),
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
        let config = openai_responses_http_config(token);
        Ok(Self::new(
            "openai",
            model_name,
            ModelProfile::for_protocol(ProtocolFamily::OpenAiResponses),
            config,
            Arc::new(ReqwestHttpClient::new()?),
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
        let config = anthropic_http_config(api_key);
        Ok(Self::new(
            "anthropic",
            model_name,
            ModelProfile::for_protocol(ProtocolFamily::AnthropicMessages),
            config,
            Arc::new(ReqwestHttpClient::new()?),
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
        let config = gemini_http_config(api_key, model_name.clone());
        Ok(Self::new(
            "gemini",
            model_name,
            ModelProfile::for_protocol(ProtocolFamily::GeminiGenerateContent),
            config,
            Arc::new(ReqwestHttpClient::new()?),
        ))
    }

    /// Create a Bedrock Converse client for a gateway endpoint.
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
}
