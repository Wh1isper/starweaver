//! HTTP config helpers for built-in provider presets.

use crate::{AuthConfig, HttpModelConfig};

fn google_cloud_model_resource(model_name: &str) -> String {
    if model_name.starts_with("projects/")
        || model_name.starts_with("models/")
        || model_name.starts_with("publishers/")
    {
        model_name.to_string()
    } else if let Some((publisher, model)) = model_name.split_once('/') {
        format!("publishers/{publisher}/models/{model}")
    } else {
        format!("publishers/google/models/{model_name}")
    }
}

fn google_cloud_base_url(location: &str) -> String {
    match location {
        "global" => "https://aiplatform.googleapis.com".to_string(),
        "us" | "eu" => format!("https://aiplatform.{location}.rep.googleapis.com"),
        location => format!("https://{location}-aiplatform.googleapis.com"),
    }
}

/// Build an Anthropic HTTP model config from an API key.
#[must_use]
pub fn anthropic_http_config(api_key: impl Into<String>) -> HttpModelConfig {
    let mut config =
        HttpModelConfig::provider_endpoint("https://api.anthropic.com/v1", "v1", "messages");
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
    let mut config =
        HttpModelConfig::provider_endpoint("https://api.openai.com/v1", "v1", "chat/completions");
    config.auth = Some(AuthConfig::Bearer {
        token: api_key.into(),
    });
    config
}

/// Build an `OpenAI` Responses HTTP model config from an API key.
#[must_use]
pub fn openai_responses_http_config(api_key: impl Into<String>) -> HttpModelConfig {
    let mut config =
        HttpModelConfig::provider_endpoint("https://api.openai.com/v1", "v1", "responses");
    config.auth = Some(AuthConfig::Bearer {
        token: api_key.into(),
    });
    config
}

/// Build an xAI Responses HTTP model config from an API key.
#[must_use]
pub fn xai_responses_http_config(api_key: impl Into<String>) -> HttpModelConfig {
    let mut config = HttpModelConfig::provider_endpoint("https://api.x.ai/v1", "v1", "responses");
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
    HttpModelConfig::provider_endpoint(
        "https://generativelanguage.googleapis.com/v1beta",
        "v1beta",
        format!("models/{model_name}:generateContent?key={}", api_key.into()),
    )
}

/// Build a Google Cloud Gemini HTTP config for Vertex AI Express Mode API keys.
#[must_use]
pub fn google_cloud_http_config(
    api_key: impl Into<String>,
    model_name: impl Into<String>,
) -> HttpModelConfig {
    let model_name = model_name.into();
    let mut config = HttpModelConfig::provider_endpoint(
        "https://aiplatform.googleapis.com",
        "v1beta1",
        format!(
            "{}:generateContent",
            google_cloud_model_resource(&model_name)
        ),
    );
    config
        .headers
        .insert("x-goog-api-key".to_string(), api_key.into());
    config
}

/// Build a Google Cloud Gemini HTTP config for project/location scoped bearer auth.
#[must_use]
pub fn google_cloud_project_http_config(
    bearer_token: impl Into<String>,
    model_name: impl Into<String>,
    project: impl AsRef<str>,
    location: impl AsRef<str>,
) -> HttpModelConfig {
    let model_name = model_name.into();
    let project = project.as_ref();
    let location = location.as_ref();
    let model_resource = google_cloud_model_resource(&model_name);
    let endpoint = if model_resource.starts_with("projects/") {
        format!("{model_resource}:generateContent")
    } else {
        format!("projects/{project}/locations/{location}/{model_resource}:generateContent")
    };
    let mut config =
        HttpModelConfig::provider_endpoint(google_cloud_base_url(location), "v1beta1", endpoint);
    config.auth = Some(AuthConfig::Bearer {
        token: bearer_token.into(),
    });
    config
}
