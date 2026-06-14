use starweaver_context::AgentContext;
use starweaver_model::ModelMessage;

pub(super) fn need_auto_compact(context: &AgentContext, messages: &[ModelMessage]) -> bool {
    let Some(context_window) = context.model_config.context_window else {
        return false;
    };
    let Some(current_tokens) = latest_request_total_tokens(messages) else {
        return false;
    };
    let threshold = context_window.saturating_mul(u64::from(
        context.model_config.compact_threshold.per_thousand(),
    )) / 1000;
    current_tokens >= threshold
}

fn latest_request_total_tokens(messages: &[ModelMessage]) -> Option<u64> {
    messages.iter().rev().find_map(|message| {
        let ModelMessage::Response(response) = message else {
            return None;
        };
        (response.usage.total_tokens > 0).then_some(response.usage.total_tokens)
    })
}
