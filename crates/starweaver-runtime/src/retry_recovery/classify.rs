//! Model error classification for retry recovery.

use starweaver_model::ModelError;

const CONTEXT_OVERFLOW_PATTERNS: &[&str] = &[
    "context_length_exceeded",
    "maximum context length",
    "max context length",
    "context window",
    "context limit",
    "context too long",
    "prompt is too long",
    "prompt too long",
    "too many tokens",
    "token count exceeds maximum",
    "exceeds maximum token",
    "exceed the maximum number of tokens",
    "input is too long",
    "input too long",
    "reduce the length of the messages",
    "reduce the size of your message",
    "messages resulted in",
    "requested tokens",
];

const OPENAI_REFERENCE_PATTERNS: &[&str] = &[
    "invalid_encrypted_content",
    "encrypted_content",
    "item_not_found",
    "item not found",
    "no item with id",
    "could not find item",
    "was provided without its required following item",
    "required following item",
    "previous_response_id",
    "previous response",
];

pub(super) fn is_openai_item_reference_error(error_text: &str) -> bool {
    let lowered = error_text.to_ascii_lowercase();
    OPENAI_REFERENCE_PATTERNS
        .iter()
        .any(|pattern| lowered.contains(pattern))
        || (lowered.contains("item")
            && (lowered.contains("not found") || lowered.contains("required following item")))
}

pub(super) fn is_context_overflow_error(error_text: &str) -> bool {
    let lowered = error_text.to_ascii_lowercase();
    if !CONTEXT_OVERFLOW_PATTERNS
        .iter()
        .any(|pattern| lowered.contains(pattern))
    {
        return false;
    }
    lowered.contains("token")
        || lowered.contains("context")
        || lowered.contains("prompt")
        || lowered.contains("message")
        || lowered.contains("input")
}

pub(super) fn model_error_text(error: &ModelError) -> String {
    let mut parts = Vec::new();
    collect_model_error_text(error, &mut parts);
    parts.join("\n")
}

fn collect_model_error_text(error: &ModelError, parts: &mut Vec<String>) {
    parts.push(format!("{error:?}"));
    parts.push(error.to_string());
    match error {
        ModelError::ProviderStatus { body, .. } => parts.push(body.to_string()),
        ModelError::RetryExhausted { source, .. } => collect_model_error_text(source, parts),
        ModelError::MessageMapping(_)
        | ModelError::ResponseParsing(_)
        | ModelError::Transport(_)
        | ModelError::RealModelRequestBlocked { .. }
        | ModelError::UnsupportedResponse(_) => {}
    }
}
