use serde_json::{Map, Value};
use starweaver_model::{ModelRequestParameters, ModelSettings, ProviderReplaySettings};

pub(super) fn compact_model_settings(
    defaults: Option<&ModelSettings>,
    inherited: Option<&ModelSettings>,
) -> Option<ModelSettings> {
    let mut settings = match (defaults, inherited) {
        (Some(defaults), Some(inherited)) => defaults.merge(inherited),
        (Some(defaults), None) => defaults.clone(),
        (None, Some(inherited)) => inherited.clone(),
        (None, None) => return None,
    };
    strip_compact_model_settings(&mut settings);
    Some(settings)
}

fn strip_compact_model_settings(settings: &mut ModelSettings) {
    settings.max_tokens = Some(settings.max_tokens.unwrap_or(4096).min(4096));
    settings.thinking = None;
    settings.provider_replay = Some(ProviderReplaySettings {
        send_item_ids: Some(false),
        include_encrypted_reasoning: Some(false),
        ..ProviderReplaySettings::default()
    });
    strip_compact_unsupported_body(&mut settings.extra_body);
    strip_unsupported_beta_header(&mut settings.extra_headers);
    if let Some(Value::Object(provider_options)) = &mut settings.provider_options {
        strip_compact_unsupported_body(provider_options);
    }
}

pub(super) fn compact_request_params(inherited: &ModelRequestParameters) -> ModelRequestParameters {
    let mut params = inherited.clone();
    params.output_schema = None;
    params.output_mode = None;
    params.thinking = None;
    params.allow_text_output = Some(true);
    strip_compact_unsupported_body(&mut params.extra_body);
    strip_compact_unsupported_body(&mut params.http.extra_body);
    strip_unsupported_beta_header(&mut params.http.headers);
    params
}

fn strip_compact_unsupported_body(body: &mut Map<String, Value>) {
    for key in [
        "anthropic_cache_tool_definitions",
        "anthropic_cache_instructions",
        "anthropic_cache_messages",
        "anthropic_cache",
        "thinking",
        "anthropic_thinking",
        "anthropic_effort",
    ] {
        body.remove(key);
    }
    strip_clear_thinking_edits(body);
}

fn strip_unsupported_beta_header(headers: &mut std::collections::BTreeMap<String, String>) {
    let Some(beta_header) = headers.get("anthropic-beta").cloned() else {
        return;
    };
    let filtered = beta_header
        .split(',')
        .map(str::trim)
        .filter(|beta| !beta.is_empty() && *beta != "interleaved-thinking-2025-05-14")
        .collect::<Vec<_>>();
    if filtered.is_empty() {
        headers.remove("anthropic-beta");
    } else {
        headers.insert("anthropic-beta".to_string(), filtered.join(","));
    }
}

fn strip_clear_thinking_edits(body: &mut Map<String, Value>) {
    let Some(Value::Object(context_management)) = body.get_mut("context_management") else {
        return;
    };
    let Some(Value::Array(edits)) = context_management.get_mut("edits") else {
        return;
    };
    edits.retain(|edit| {
        !edit
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| kind.contains("clear_thinking"))
    });
    if edits.is_empty() {
        context_management.remove("edits");
    }
    if context_management.is_empty() {
        body.remove("context_management");
    }
}
