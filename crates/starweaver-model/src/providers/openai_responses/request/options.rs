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
        let previous_response_id =
            provider_replay.and_then(|replay| replay.previous_response_id.clone());
        let conversation_id = provider_replay.and_then(|replay| replay.conversation_id.clone());
        let send_item_ids = provider_replay
            .and_then(|replay| replay.send_item_ids)
            .unwrap_or(true);
        let include_encrypted_reasoning = provider_replay
            .and_then(|replay| replay.include_encrypted_reasoning)
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
