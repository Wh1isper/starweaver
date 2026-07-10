use serde_json::{Map, Value, json};

use crate::{
    ModelError,
    adapter::{ModelRequestContext, ModelRequestParameters},
    profile::ProtocolFamily,
    settings::{
        GoogleCloudServiceTier, ModelSettings, ServiceTier, format_openai_prompt_cache_key,
        supports_automatic_openai_prompt_cache_key,
    },
    transport::{HttpRequest, HttpRequestOptions, extend_headers_case_insensitive},
};

use super::ProtocolModelClient;

impl ProtocolModelClient {
    pub(super) fn request_options(
        &self,
        context: &ModelRequestContext,
        settings: Option<&ModelSettings>,
        params: &ModelRequestParameters,
    ) -> HttpRequestOptions {
        let mut options = HttpRequestOptions::default();
        if let Some(settings) = settings {
            if matches!(self.profile.protocol, ProtocolFamily::AnthropicMessages)
                && let Some(anthropic) = &settings.provider_settings.anthropic
                && !anthropic.betas.is_empty()
            {
                options
                    .headers
                    .insert("anthropic-beta".to_string(), anthropic.betas.join(","));
            }
            if self.provider_name == "google-cloud"
                && matches!(self.profile.protocol, ProtocolFamily::GeminiGenerateContent)
            {
                extend_headers_case_insensitive(
                    &mut options.headers,
                    google_cloud_service_tier_headers(settings),
                );
            }
            if let Some(gateway) = &settings.provider_settings.gateway {
                if let Some(x_session_id) = &gateway.x_session_id {
                    extend_headers_case_insensitive(
                        &mut options.headers,
                        [("x-session-id".to_string(), x_session_id.clone())],
                    );
                }
                extend_headers_case_insensitive(
                    &mut options.headers,
                    gateway.extra_headers.clone(),
                );
            }
            extend_headers_case_insensitive(&mut options.headers, settings.extra_headers.clone());
            options.extra_body.extend(settings.extra_body.clone());
            options.timeout_ms = settings.timeout_ms;
        }
        extend_headers_case_insensitive(&mut options.headers, params.http.headers.clone());
        options.extra_body.extend(params.http.extra_body.clone());
        options.endpoint_url.clone_from(&params.http.endpoint_url);
        options.timeout_ms = params.http.timeout_ms.or(options.timeout_ms);
        options.metadata.extend(params.http.metadata.clone());
        options.extra_body.extend(params.extra_body.clone());
        options.metadata.extend(params.metadata.clone());
        options.metadata.extend(context.llm_trace_metadata.clone());
        options.metadata.insert(
            "starweaver.run_id".to_string(),
            json!(context.run_id.as_str()),
        );
        options.metadata.insert(
            "starweaver.conversation_id".to_string(),
            json!(context.conversation_id.as_str()),
        );
        if let Some(codex) = settings.and_then(|settings| settings.provider_settings.codex.as_ref())
        {
            if let Some(session_id) = &codex.session_id {
                options
                    .metadata
                    .insert("provider.codex.session_id".to_string(), json!(session_id));
            }
            if let Some(thread_id) = &codex.thread_id {
                options
                    .metadata
                    .insert("provider.codex.thread_id".to_string(), json!(thread_id));
            }
        }
        options
    }

    pub(super) fn finalize_http_request(
        &self,
        request: &mut HttpRequest,
    ) -> Result<(), ModelError> {
        let metadata = &request.metadata;
        let Some(body) = request.body.as_object_mut() else {
            return Ok(());
        };
        if matches!(
            self.profile.protocol,
            ProtocolFamily::OpenAiResponses | ProtocolFamily::OpenAiChatCompletions
        ) {
            if self.provider_name != "codex" {
                apply_openai_prompt_cache_metadata(
                    metadata,
                    body,
                    supports_automatic_openai_prompt_cache_key(&self.model_name),
                );
            }
            validate_openai_prompt_cache_fields(&self.model_name, body)?;
        }
        if self.provider_name == "google-cloud"
            && matches!(self.profile.protocol, ProtocolFamily::GeminiGenerateContent)
        {
            body.remove("serviceTier");
        }
        Ok(())
    }
}

fn validate_openai_prompt_cache_fields(
    model: &str,
    body: &Map<String, Value>,
) -> Result<(), ModelError> {
    if body.contains_key("prompt_cache_retention") && body.contains_key("prompt_cache_options") {
        return Err(ModelError::MessageMapping(
            "OpenAI prompt_cache_retention and prompt_cache_options cannot both be sent"
                .to_string(),
        ));
    }
    if body.contains_key("prompt_cache_options")
        && !crate::settings::supports_openai_prompt_cache_breakpoints(model)
    {
        return Err(ModelError::MessageMapping(format!(
            "model {model} does not support OpenAI prompt_cache_options"
        )));
    }
    Ok(())
}

fn google_cloud_service_tier_headers(settings: &ModelSettings) -> Vec<(String, String)> {
    let tier = settings
        .provider_settings
        .google
        .as_ref()
        .and_then(|google| google.cloud_service_tier.as_ref())
        .cloned()
        .or_else(|| {
            settings
                .service_tier
                .as_ref()
                .map(map_service_tier_to_google_cloud)
        });
    match tier {
        None | Some(GoogleCloudServiceTier::PtThenOnDemand) => Vec::new(),
        Some(GoogleCloudServiceTier::PtOnly) => vec![(
            "X-Vertex-AI-LLM-Request-Type".to_string(),
            "dedicated".to_string(),
        )],
        Some(GoogleCloudServiceTier::OnDemand) => vec![(
            "X-Vertex-AI-LLM-Request-Type".to_string(),
            "shared".to_string(),
        )],
        Some(GoogleCloudServiceTier::PtThenFlex) => vec![(
            "X-Vertex-AI-LLM-Shared-Request-Type".to_string(),
            "flex".to_string(),
        )],
        Some(GoogleCloudServiceTier::PtThenPriority) => vec![(
            "X-Vertex-AI-LLM-Shared-Request-Type".to_string(),
            "priority".to_string(),
        )],
        Some(GoogleCloudServiceTier::FlexOnly) => vec![
            (
                "X-Vertex-AI-LLM-Request-Type".to_string(),
                "shared".to_string(),
            ),
            (
                "X-Vertex-AI-LLM-Shared-Request-Type".to_string(),
                "flex".to_string(),
            ),
        ],
        Some(GoogleCloudServiceTier::PriorityOnly) => vec![
            (
                "X-Vertex-AI-LLM-Request-Type".to_string(),
                "shared".to_string(),
            ),
            (
                "X-Vertex-AI-LLM-Shared-Request-Type".to_string(),
                "priority".to_string(),
            ),
        ],
    }
}

const fn map_service_tier_to_google_cloud(tier: &ServiceTier) -> GoogleCloudServiceTier {
    match tier {
        ServiceTier::Auto | ServiceTier::Default => GoogleCloudServiceTier::PtThenOnDemand,
        ServiceTier::Flex => GoogleCloudServiceTier::PtThenFlex,
        ServiceTier::Priority => GoogleCloudServiceTier::PtThenPriority,
    }
}

fn apply_openai_prompt_cache_metadata(
    metadata: &Map<String, Value>,
    extra_body: &mut Map<String, Value>,
    auto_affinity_key: bool,
) {
    if !extra_body.contains_key("prompt_cache_key") {
        if let Some(key) = metadata_string(metadata, &["starweaver.prompt_cache_key"]) {
            extra_body.insert("prompt_cache_key".to_string(), json!(key));
        } else if auto_affinity_key
            && let Some(affinity_id) =
                metadata_string(metadata, &["starweaver.prompt_cache_affinity_id"])
                    .and_then(format_openai_prompt_cache_key)
        {
            extra_body.insert("prompt_cache_key".to_string(), json!(affinity_id));
        }
    }
    if !extra_body.contains_key("prompt_cache_retention")
        && let Some(retention) = metadata_string(metadata, &["starweaver.prompt_cache_retention"])
    {
        extra_body.insert("prompt_cache_retention".to_string(), json!(retention));
    }
}

fn metadata_string<'a>(metadata: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| metadata.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
}
