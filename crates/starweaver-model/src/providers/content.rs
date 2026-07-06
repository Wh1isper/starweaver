//! Provider content part mapping helpers.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};

use crate::{message::ContentPart, parse_data_url};

#[cfg(test)]
pub fn text_from_content(content: &[ContentPart]) -> String {
    content
        .iter()
        .filter_map(|part| match part {
            ContentPart::Text { text } => Some(text.as_str()),
            ContentPart::ImageUrl { .. }
            | ContentPart::FileUrl { .. }
            | ContentPart::Binary { .. }
            | ContentPart::ResourceRef { .. }
            | ContentPart::DataUrl { .. } => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

pub fn openai_chat_content(content: &[ContentPart]) -> Value {
    if content.len() == 1
        && let ContentPart::Text { text } = &content[0]
    {
        return json!(text);
    }
    Value::Array(
        content
            .iter()
            .map(|part| match part {
                ContentPart::Text { text } => json!({"type": "text", "text": text}),
                ContentPart::ImageUrl { url } => {
                    json!({"type": "image_url", "image_url": {"url": url}})
                }
                ContentPart::FileUrl { url, media_type } => json!({
                    "type": "file",
                    "file": {"file_data": url, "media_type": media_type}
                }),
                ContentPart::Binary { data, media_type } => {
                    if media_type.starts_with("image/") {
                        json!({"type": "image_url", "image_url": {"url": data_url(media_type, data)}})
                    } else {
                        json!({"type": "file", "file": {"file_data": data_url(media_type, data), "media_type": media_type}})
                    }
                }
                ContentPart::ResourceRef { uri, media_type, .. } => {
                    if media_type.starts_with("image/") {
                        json!({"type": "image_url", "image_url": {"url": uri}})
                    } else {
                        json!({"type": "file", "file": {"file_data": uri, "media_type": media_type}})
                    }
                }
                ContentPart::DataUrl { data_url, media_type } => {
                    if media_type.starts_with("image/") {
                        json!({"type": "image_url", "image_url": {"url": data_url}})
                    } else {
                        json!({"type": "file", "file": {"file_data": data_url, "media_type": media_type}})
                    }
                }
            })
            .collect(),
    )
}

pub fn openai_responses_content(content: &[ContentPart]) -> Vec<Value> {
    content
        .iter()
        .map(|part| match part {
            ContentPart::Text { text } => json!({"type": "input_text", "text": text}),
            ContentPart::ImageUrl { url } => json!({"type": "input_image", "image_url": url}),
            ContentPart::FileUrl { url, media_type } => json!({
                "type": "input_file",
                "file_url": url,
                "media_type": media_type
            }),
            ContentPart::Binary { data, media_type } => {
                if media_type.starts_with("image/") {
                    json!({"type": "input_image", "image_url": data_url(media_type, data)})
                } else {
                    json!({"type": "input_file", "file_url": data_url(media_type, data), "media_type": media_type})
                }
            }
            ContentPart::ResourceRef { uri, media_type, .. } => {
                if media_type.starts_with("image/") {
                    json!({"type": "input_image", "image_url": uri})
                } else {
                    json!({"type": "input_file", "file_url": uri, "media_type": media_type})
                }
            }
            ContentPart::DataUrl { data_url, media_type } => {
                if media_type.starts_with("image/") {
                    json!({"type": "input_image", "image_url": data_url})
                } else {
                    json!({"type": "input_file", "file_url": data_url, "media_type": media_type})
                }
            }
        })
        .collect()
}

pub fn gemini_parts_from_content(content: &[ContentPart]) -> Vec<Value> {
    content
        .iter()
        .map(|part| match part {
            ContentPart::Text { text } => json!({"text": text}),
            ContentPart::ImageUrl { url } => json!({
                "fileData": {"fileUri": url, "mimeType": "image/*"}
            }),
            ContentPart::FileUrl { url, media_type } => json!({
                "fileData": {"fileUri": url, "mimeType": media_type}
            }),
            ContentPart::Binary { data, media_type } => json!({
                "inlineData": {"data": base64_payload(data), "mimeType": media_type}
            }),
            ContentPart::ResourceRef {
                uri, media_type, ..
            } => json!({
                "fileData": {"fileUri": uri, "mimeType": media_type}
            }),
            ContentPart::DataUrl {
                data_url,
                media_type,
            } => gemini_data_url(data_url, media_type),
        })
        .collect()
}

pub fn bedrock_content_from_content(content: &[ContentPart]) -> Vec<Value> {
    content
        .iter()
        .map(|part| match part {
            ContentPart::Text { text } => json!({"text": text}),
            ContentPart::ImageUrl { url } => json!({"image": {"source": {"bytes": url}}}),
            ContentPart::FileUrl { url, media_type } => json!({
                "document": {
                    "format": media_type,
                    "source": {"bytes": url},
                }
            }),
            ContentPart::Binary { data, media_type } => {
                if media_type.starts_with("image/") {
                    json!({"image": {"format": bedrock_media_format(media_type), "source": {"bytes": base64_payload(data)}}})
                } else {
                    json!({"document": {"format": bedrock_media_format(media_type), "source": {"bytes": base64_payload(data)}}})
                }
            }
            ContentPart::ResourceRef {
                uri, media_type, ..
            } => {
                if media_type.starts_with("image/") {
                    json!({"image": {"format": bedrock_media_format(media_type), "source": {"bytes": uri}}})
                } else {
                    json!({"document": {"format": bedrock_media_format(media_type), "source": {"bytes": uri}}})
                }
            }
            ContentPart::DataUrl {
                data_url,
                media_type,
            } => bedrock_data_url(data_url, media_type),
        })
        .collect()
}

fn data_url(media_type: &str, data: &[u8]) -> String {
    format!("data:{media_type};base64,{}", base64_payload(data))
}

fn gemini_data_url(data_url: &str, fallback_media_type: &str) -> Value {
    parse_data_url(data_url).map_or_else(
        |_| {
            json!({
                "fileData": {"fileUri": data_url, "mimeType": fallback_media_type}
            })
        },
        |parsed| {
            json!({
                "inlineData": {
                    "data": base64_payload(&parsed.data),
                    "mimeType": parsed.media_type,
                }
            })
        },
    )
}

fn bedrock_data_url(data_url: &str, fallback_media_type: &str) -> Value {
    parse_data_url(data_url).map_or_else(
        |_| {
            if fallback_media_type.starts_with("image/") {
                json!({"image": {"format": bedrock_media_format(fallback_media_type), "source": {"bytes": data_url}}})
            } else {
                json!({"document": {"format": bedrock_media_format(fallback_media_type), "source": {"bytes": data_url}}})
            }
        },
        |parsed| {
            if parsed.media_type.starts_with("image/") {
                json!({"image": {"format": bedrock_media_format(&parsed.media_type), "source": {"bytes": base64_payload(&parsed.data)}}})
            } else {
                json!({"document": {"format": bedrock_media_format(&parsed.media_type), "source": {"bytes": base64_payload(&parsed.data)}}})
            }
        },
    )
}

fn base64_payload(data: &[u8]) -> String {
    STANDARD.encode(data)
}

fn bedrock_media_format(media_type: &str) -> &str {
    media_type
        .strip_prefix("image/")
        .or_else(|| media_type.strip_prefix("application/"))
        .unwrap_or(media_type)
}
