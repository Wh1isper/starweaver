use serde_json::{json, Map, Value};

use crate::{
    adapter::{ModelRequestContext, ModelRequestParameters},
    profile::ProtocolFamily,
    settings::{
        format_openai_prompt_cache_key, supports_automatic_openai_prompt_cache_key, ModelSettings,
    },
    transport::{extend_headers_case_insensitive, HttpRequest, HttpRequestOptions},
};

use super::ProtocolModelClient;

const OPENAI_REPLAY_ALIASES: &[&str] = &[
    "openai_previous_response_id",
    "openai_conversation_id",
    "openai_send_reasoning_ids",
    "openai_include_encrypted_reasoning",
];
impl ProtocolModelClient {
    pub(super) fn request_options(
        &self,
        context: &ModelRequestContext,
        settings: Option<&ModelSettings>,
        params: &ModelRequestParameters,
    ) -> HttpRequestOptions {
        let mut options = HttpRequestOptions::default();
        if let Some(settings) = settings {
            if matches!(self.profile.protocol, ProtocolFamily::AnthropicMessages) {
                if let Some(anthropic) = &settings.provider_settings.anthropic {
                    if !anthropic.betas.is_empty() {
                        options
                            .headers
                            .insert("anthropic-beta".to_string(), anthropic.betas.join(","));
                    }
                }
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
        self.apply_protocol_request_options(context, &mut options);
        options
    }

    fn apply_protocol_request_options(
        &self,
        _context: &ModelRequestContext,
        options: &mut HttpRequestOptions,
    ) {
        if matches!(self.profile.protocol, ProtocolFamily::OpenAiResponses) {
            strip_openai_replay_aliases(&mut options.extra_body);
        }
    }

    pub(super) fn finalize_http_request(&self, request: &mut HttpRequest) {
        let metadata = &request.metadata;
        let Some(body) = request.body.as_object_mut() else {
            return;
        };
        if matches!(self.profile.protocol, ProtocolFamily::OpenAiResponses) {
            strip_openai_replay_aliases(body);
        }
        if self.provider_name != "codex"
            && matches!(
                self.profile.protocol,
                ProtocolFamily::OpenAiResponses | ProtocolFamily::OpenAiChatCompletions
            )
        {
            apply_openai_prompt_cache_metadata(
                metadata,
                body,
                supports_automatic_openai_prompt_cache_key(&self.model_name),
            );
        }
    }
}

fn strip_openai_replay_aliases(extra_body: &mut Map<String, Value>) {
    for key in OPENAI_REPLAY_ALIASES {
        extra_body.remove(*key);
    }
}

fn apply_openai_prompt_cache_metadata(
    metadata: &Map<String, Value>,
    extra_body: &mut Map<String, Value>,
    auto_session_key: bool,
) {
    // Compatibility fallback only. New runtime routing should set prompt-cache
    // keys through typed OpenAI provider settings before this finalizer runs.
    if !extra_body.contains_key("prompt_cache_key") {
        if let Some(key) = metadata_string(
            metadata,
            &["starweaver.prompt_cache_key", "openai.prompt_cache_key"],
        ) {
            extra_body.insert("prompt_cache_key".to_string(), json!(key));
        } else if auto_session_key {
            if let Some(session_id) =
                metadata_string(metadata, &["starweaver.session_id", "cli.session_id"])
                    .and_then(format_openai_prompt_cache_key)
            {
                extra_body.insert("prompt_cache_key".to_string(), json!(session_id));
            }
        }
    }
    if !extra_body.contains_key("prompt_cache_retention") {
        if let Some(retention) = metadata_string(
            metadata,
            &[
                "starweaver.prompt_cache_retention",
                "openai.prompt_cache_retention",
            ],
        ) {
            extra_body.insert("prompt_cache_retention".to_string(), json!(retention));
        }
    }
}

fn metadata_string<'a>(metadata: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| metadata.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
}
