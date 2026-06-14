use std::sync::Arc;

use crate::{
    profile::{ModelProfile, ProtocolFamily},
    transport::{AuthConfig, DynHttpClient, HttpModelConfig, ReqwestHttpClient},
    ModelError,
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
        let mut config = HttpModelConfig::new("https://api.openai.com/v1", "chat/completions");
        config.auth = Some(AuthConfig::Bearer {
            token: token.into(),
        });
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
        let mut config = HttpModelConfig::new("https://api.openai.com/v1", "responses");
        config.auth = Some(AuthConfig::Bearer {
            token: token.into(),
        });
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
        let mut config = HttpModelConfig::new("https://api.anthropic.com/v1", "messages");
        config.auth = Some(AuthConfig::Header {
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
        let config = HttpModelConfig::new(
            "https://generativelanguage.googleapis.com/v1beta",
            format!("models/{model_name}:generateContent?key={}", api_key.into()),
        );
        Ok(Self::new(
            "gemini",
            model_name,
            ModelProfile::for_protocol(ProtocolFamily::GeminiGenerateContent),
            config,
            Arc::new(ReqwestHttpClient::new()?),
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
}
