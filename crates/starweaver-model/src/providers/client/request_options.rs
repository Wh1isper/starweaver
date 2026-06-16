use serde_json::{json, Map, Value};

use crate::{
    adapter::{ModelRequestContext, ModelRequestParameters},
    profile::ProtocolFamily,
    settings::ModelSettings,
    transport::{HttpRequest, HttpRequestOptions},
};

use super::ProtocolModelClient;

const OPENAI_REPLAY_ALIASES: &[&str] = &[
    "openai_previous_response_id",
    "openai_conversation_id",
    "openai_send_reasoning_ids",
    "openai_include_encrypted_reasoning",
];
const OPENAI_PROMPT_CACHE_KEY_LIMIT: usize = 64;

impl ProtocolModelClient {
    pub(super) fn request_options(
        &self,
        context: &ModelRequestContext,
        settings: Option<&ModelSettings>,
        params: &ModelRequestParameters,
    ) -> HttpRequestOptions {
        let mut options = params.http.clone();
        if let Some(settings) = settings {
            options.headers.extend(settings.extra_headers.clone());
            options.extra_body.extend(settings.extra_body.clone());
            options.timeout_ms = options.timeout_ms.or(settings.timeout_ms);
        }
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
    if !extra_body.contains_key("prompt_cache_key") {
        if let Some(key) = metadata_string(
            metadata,
            &["starweaver.prompt_cache_key", "openai.prompt_cache_key"],
        ) {
            extra_body.insert("prompt_cache_key".to_string(), json!(key));
        } else if auto_session_key {
            if let Some(session_id) =
                metadata_string(metadata, &["starweaver.session_id", "cli.session_id"])
                    .and_then(session_prompt_cache_key)
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

fn supports_automatic_openai_prompt_cache_key(model_name: &str) -> bool {
    let model = model_name.trim().to_ascii_lowercase();
    model.starts_with("gpt-")
        || model.starts_with("chatgpt-")
        || model.starts_with("o1")
        || model.starts_with("o3")
        || model.starts_with("o4")
}

fn session_prompt_cache_key(session_id: &str) -> Option<String> {
    let mut key = String::from("sw_");
    for ch in session_id.trim().chars() {
        if key.len() >= OPENAI_PROMPT_CACHE_KEY_LIMIT {
            break;
        }
        key.push(
            if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.') {
                ch
            } else {
                '_'
            },
        );
    }
    (key.len() > "sw_".len()).then_some(key)
}
