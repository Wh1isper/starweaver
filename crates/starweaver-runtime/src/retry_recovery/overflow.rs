//! Context-overflow recovery by trimming tool returns and media payloads.

use serde_json::{Map, Value};
use starweaver_model::{
    ContentPart, ModelMessage, ModelRequestPart, ModelResponsePart, ToolReturnPart,
};

const TOOL_RETURN_MAX_CHARS: usize = 500;
const TOOL_RETURN_KEEP_HEAD: usize = 200;
const TOOL_RETURN_KEEP_TAIL: usize = 200;

const MEDIA_REMOVED_REMINDER: &str = "<system-reminder>Media content was removed during retry recovery because the previous request exceeded the model context limit. If the media is still needed, ask the user to attach it again or inspect it with a focused tool call.</system-reminder>";
const RESPONSE_MEDIA_REMOVED_TEXT: &str = "<system-reminder>Assistant media content was removed during retry recovery because the previous request exceeded the model context limit.</system-reminder>";

/// Trim older large tool returns and remove image/video media in place.
pub fn heal_context_overflow_history(history: &mut [ModelMessage]) -> bool {
    let trimmed = trim_tool_returns(history) > 0;
    let stripped = strip_image_video_media(history);
    trimmed || stripped
}

fn trim_tool_returns(history: &mut [ModelMessage]) -> usize {
    let mut trimmed = 0usize;

    for message in history {
        let ModelMessage::Request(request) = message else {
            continue;
        };
        for part in &mut request.parts {
            let ModelRequestPart::ToolReturn(tool_return) = part else {
                continue;
            };
            if truncate_tool_return_content(tool_return) {
                trimmed = trimmed.saturating_add(1);
            }
        }
    }

    trimmed
}

fn truncate_tool_return_content(tool_return: &mut ToolReturnPart) -> bool {
    let content = tool_content_text(&tool_return.content);
    if content.chars().count() <= TOOL_RETURN_MAX_CHARS {
        return false;
    }
    let original_chars = content.chars().count();
    tool_return.content = Value::String(truncate_tool_content(&content));
    tool_return.metadata.insert(
        "starweaver_retry_recovery_truncated".to_string(),
        Value::Bool(true),
    );
    tool_return.metadata.insert(
        "starweaver_retry_recovery_original_chars".to_string(),
        serde_json::json!(original_chars),
    );

    if let Some(user_content) = &mut tool_return.user_content {
        let user_text = tool_content_text(user_content);
        if user_text.chars().count() > TOOL_RETURN_MAX_CHARS {
            *user_content = Value::String(truncate_tool_content(&user_text));
        }
    }

    true
}

fn tool_content_text(value: &Value) -> String {
    value
        .as_str()
        .map_or_else(|| value.to_string(), str::to_string)
}

fn truncate_tool_content(content: &str) -> String {
    let total = content.chars().count();
    if total <= TOOL_RETURN_MAX_CHARS {
        return content.to_string();
    }
    let head = content
        .chars()
        .take(TOOL_RETURN_KEEP_HEAD)
        .collect::<String>();
    let tail = content
        .chars()
        .rev()
        .take(TOOL_RETURN_KEEP_TAIL)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    let truncated = total.saturating_sub(TOOL_RETURN_KEEP_HEAD + TOOL_RETURN_KEEP_TAIL);
    format!("{head}\n[... {truncated} chars truncated ...]\n{tail}")
}

fn strip_image_video_media(history: &mut [ModelMessage]) -> bool {
    let mut changed = false;
    for message in history {
        match message {
            ModelMessage::Request(request) => {
                for part in &mut request.parts {
                    match part {
                        ModelRequestPart::UserPrompt { content, .. } => {
                            for item in content {
                                changed |= replace_media_content_part(item);
                            }
                        }
                        ModelRequestPart::ToolReturn(tool_return) => {
                            changed |= replace_media_value(&mut tool_return.content);
                        }
                        ModelRequestPart::SystemPrompt { .. }
                        | ModelRequestPart::RetryPrompt { .. }
                        | ModelRequestPart::Instruction { .. } => {}
                    }
                }
            }
            ModelMessage::Response(response) => {
                for part in &mut response.parts {
                    if matches!(part, ModelResponsePart::File { media_type, .. } if is_image_or_video_media_type(media_type))
                    {
                        *part = ModelResponsePart::Text {
                            text: RESPONSE_MEDIA_REMOVED_TEXT.to_string(),
                        };
                        changed = true;
                    }
                }
            }
        }
    }
    changed
}

fn replace_media_content_part(part: &mut ContentPart) -> bool {
    if is_image_or_video_content_part(part) {
        *part = ContentPart::Text {
            text: MEDIA_REMOVED_REMINDER.to_string(),
        };
        return true;
    }
    false
}

fn is_image_or_video_content_part(part: &ContentPart) -> bool {
    match part {
        ContentPart::ImageUrl { .. } => true,
        ContentPart::FileUrl { media_type, .. }
        | ContentPart::Binary { media_type, .. }
        | ContentPart::ResourceRef { media_type, .. }
        | ContentPart::DataUrl { media_type, .. } => is_image_or_video_media_type(media_type),
        ContentPart::CachePoint { .. } | ContentPart::Text { .. } => false,
    }
}

fn replace_media_value(value: &mut Value) -> bool {
    match value {
        Value::Array(items) => {
            let mut changed = false;
            for item in items {
                changed |= replace_media_value(item);
            }
            changed
        }
        Value::Object(object) => {
            if value_object_looks_like_image_or_video(object) {
                *value = Value::String(MEDIA_REMOVED_REMINDER.to_string());
                true
            } else {
                let mut changed = false;
                for item in object.values_mut() {
                    changed |= replace_media_value(item);
                }
                changed
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => false,
    }
}

fn value_object_looks_like_image_or_video(object: &Map<String, Value>) -> bool {
    let kind = object.get("kind").and_then(Value::as_str);
    if matches!(kind, Some("image_url" | "image" | "video")) {
        return true;
    }
    object
        .get("media_type")
        .and_then(Value::as_str)
        .is_some_and(is_image_or_video_media_type)
        && (object.contains_key("data")
            || object.contains_key("data_url")
            || object.contains_key("url")
            || object.contains_key("uri"))
}

fn is_image_or_video_media_type(media_type: &str) -> bool {
    media_type.starts_with("image/") || media_type.starts_with("video/")
}
