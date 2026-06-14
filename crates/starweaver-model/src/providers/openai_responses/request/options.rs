use serde_json::Value;

use crate::ModelSettings;

#[derive(Clone, Debug)]
pub(super) struct OpenAiReplayOptions {
    pub(super) previous_response_id: Option<String>,
    pub(super) conversation_id: Option<String>,
    pub(super) send_item_ids: bool,
    pub(super) include_encrypted_reasoning: bool,
}

impl OpenAiReplayOptions {
    pub(super) fn from_settings(settings: Option<&ModelSettings>) -> Self {
        let provider_replay = settings.and_then(|settings| settings.provider_replay.as_ref());
        let previous_response_id = provider_replay
            .and_then(|replay| replay.previous_response_id.clone())
            .or_else(|| provider_setting_string(settings, &["openai_previous_response_id"]));
        let conversation_id = provider_replay
            .and_then(|replay| replay.conversation_id.clone())
            .or_else(|| provider_setting_string(settings, &["openai_conversation_id"]));
        let send_item_ids = provider_replay
            .and_then(|replay| replay.send_item_ids)
            .or_else(|| provider_setting_bool(settings, &["openai_send_reasoning_ids"]))
            .unwrap_or(true);
        let include_encrypted_reasoning = provider_replay
            .and_then(|replay| replay.include_encrypted_reasoning)
            .or_else(|| provider_setting_bool(settings, &["openai_include_encrypted_reasoning"]))
            .unwrap_or_else(|| {
                send_item_ids
                    && settings
                        .and_then(|settings| settings.thinking.as_ref())
                        .is_some()
            });
        Self {
            previous_response_id,
            conversation_id,
            send_item_ids,
            include_encrypted_reasoning,
        }
    }
}

fn provider_setting_string(settings: Option<&ModelSettings>, keys: &[&str]) -> Option<String> {
    let settings = settings?;
    keys.iter()
        .find_map(|key| setting_value(settings, key).and_then(Value::as_str))
        .map(str::to_string)
}

fn provider_setting_bool(settings: Option<&ModelSettings>, keys: &[&str]) -> Option<bool> {
    let settings = settings?;
    keys.iter()
        .find_map(|key| setting_value(settings, key).and_then(Value::as_bool))
}

fn setting_value<'a>(settings: &'a ModelSettings, key: &str) -> Option<&'a Value> {
    settings
        .provider_options
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|object| object.get(key))
        .or_else(|| settings.extra_body.get(key))
}
