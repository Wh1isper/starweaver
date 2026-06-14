//! HTTP config helpers for built-in provider presets.

use crate::{AuthConfig, HttpModelConfig};

/// Build an Anthropic HTTP model config from an API key.
#[must_use]
pub fn anthropic_http_config(api_key: impl Into<String>) -> HttpModelConfig {
    let mut config = HttpModelConfig::new("https://api.anthropic.com/v1", "messages");
    config.auth = Some(AuthConfig::Header {
        name: "x-api-key".to_string(),
        value: api_key.into(),
    });
    config
        .headers
        .insert("anthropic-version".to_string(), "2023-06-01".to_string());
    config
}

/// Build an `OpenAI` Chat Completions HTTP model config from an API key.
#[must_use]
pub fn openai_chat_http_config(api_key: impl Into<String>) -> HttpModelConfig {
    let mut config = HttpModelConfig::new("https://api.openai.com/v1", "chat/completions");
    config.auth = Some(AuthConfig::Bearer {
        token: api_key.into(),
    });
    config
}

/// Build an `OpenAI` Responses HTTP model config from an API key.
#[must_use]
pub fn openai_responses_http_config(api_key: impl Into<String>) -> HttpModelConfig {
    let mut config = HttpModelConfig::new("https://api.openai.com/v1", "responses");
    config.auth = Some(AuthConfig::Bearer {
        token: api_key.into(),
    });
    config
}

/// Build a Gemini HTTP model config from an API key and model name.
#[must_use]
pub fn gemini_http_config(
    api_key: impl Into<String>,
    model_name: impl Into<String>,
) -> HttpModelConfig {
    let model_name = model_name.into();
    HttpModelConfig::new(
        "https://generativelanguage.googleapis.com/v1beta",
        format!("models/{model_name}:generateContent?key={}", api_key.into()),
    )
}
