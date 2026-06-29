use serde_json::Value;
use starweaver_stream::{DisplayMessage, DisplayMessageKind};

pub fn run_output_preview(messages: &[DisplayMessage]) -> Option<String> {
    messages.iter().rev().find_map(message_output_preview)
}

fn message_output_preview(message: &DisplayMessage) -> Option<String> {
    if is_internal_run_message(message.kind) {
        return None;
    }
    message
        .payload
        .get("output")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .or_else(|| message.preview.clone())
}

const fn is_internal_run_message(kind: DisplayMessageKind) -> bool {
    matches!(
        kind,
        DisplayMessageKind::CompactionStarted | DisplayMessageKind::CompactionCompleted
    )
}
