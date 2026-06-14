//! Anthropic content block mapping.

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Value};

use crate::{
    media::parse_data_url,
    message::{ContentPart, ToolReturnPart},
    ModelError,
};

pub(super) fn anthropic_content_from_content(
    content: &[ContentPart],
) -> Result<Vec<Value>, ModelError> {
    content
        .iter()
        .map(|part| match part {
            ContentPart::Text { text } => Ok(json!({"type": "text", "text": text})),
            ContentPart::ImageUrl { url } => Ok(anthropic_image_url(url)),
            ContentPart::FileUrl { url, media_type } => anthropic_url_content(url, media_type),
            ContentPart::Binary { data, media_type } => anthropic_binary_content(data, media_type),
            ContentPart::ResourceRef {
                uri, media_type, ..
            } => anthropic_url_content(uri, media_type),
            ContentPart::DataUrl { data_url, .. } => {
                let parsed = parse_data_url(data_url).map_err(|error| {
                    ModelError::MessageMapping(format!("invalid Anthropic data URL: {error}"))
                })?;
                anthropic_binary_content(&parsed.data, &parsed.media_type)
            }
        })
        .collect()
}

pub(super) fn anthropic_tool_result(tool_return: &ToolReturnPart) -> Value {
    let mut result = json!({
        "type": "tool_result",
        "tool_use_id": tool_return.tool_call_id,
        "content": tool_return.content.to_string(),
        "is_error": tool_return.is_error,
    });
    if let Some(cache_control) = tool_return.metadata.get("cache_control") {
        result["cache_control"] = cache_control.clone();
    }
    result
}

fn anthropic_url_content(url: &str, media_type: &str) -> Result<Value, ModelError> {
    if media_type.starts_with("image/") {
        return Ok(anthropic_image_url(url));
    }
    if media_type.starts_with("audio/") || media_type.starts_with("video/") {
        return Err(ModelError::MessageMapping(format!(
            "Anthropic Messages does not support media type {media_type}"
        )));
    }
    Ok(anthropic_document_url(url))
}

fn anthropic_image_url(url: &str) -> Value {
    json!({
        "type": "image",
        "source": {"type": "url", "url": url},
    })
}

fn anthropic_document_url(url: &str) -> Value {
    json!({
        "type": "document",
        "source": {"type": "url", "url": url},
    })
}

fn anthropic_binary_content(data: &[u8], media_type: &str) -> Result<Value, ModelError> {
    if media_type.starts_with("image/") {
        return Ok(json!({
            "type": "image",
            "source": {
                "type": "base64",
                "media_type": media_type,
                "data": STANDARD.encode(data),
            },
        }));
    }
    if media_type.starts_with("audio/") || media_type.starts_with("video/") {
        return Err(ModelError::MessageMapping(format!(
            "Anthropic Messages does not support media type {media_type}"
        )));
    }
    if media_type == "text/plain" {
        return Ok(json!({
            "type": "document",
            "source": {
                "type": "text",
                "media_type": media_type,
                "data": String::from_utf8_lossy(data).into_owned(),
            },
        }));
    }
    Ok(json!({
        "type": "document",
        "source": {
            "type": "base64",
            "media_type": media_type,
            "data": STANDARD.encode(data),
        },
    }))
}
