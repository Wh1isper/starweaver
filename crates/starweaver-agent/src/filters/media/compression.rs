//! Media compression filtering.

use serde_json::{Map, Value, json};
use starweaver_context::AgentContext;
use starweaver_model::{
    ContentPart, ModelMessage, ModelRequestPart, detect_media_kind, parse_data_url,
};
use starweaver_runtime::AgentRunState;

use crate::{
    filters::message::request_metadata_mut,
    media_compression::{
        compress_data_url_object, compress_image_to_model_limit, compression_failed_message,
        data_url, image_exceeds_model_limits, image_within_model_limits,
        oversized_after_compression_message,
    },
};

use super::policy::media_policy_from_state_and_context;

pub(in crate::filters) fn media_compress_filter(
    state: &AgentRunState,
    context: &AgentContext,
    mut messages: Vec<ModelMessage>,
) -> Vec<ModelMessage> {
    let policy = media_policy_from_state_and_context(state, context);
    let limits = ImageLimits {
        max_encoded_bytes: policy.max_inline_base64_bytes.unwrap_or(0),
        max_dimension: policy
            .max_image_dimension
            .map_or(0, |value| usize::try_from(value).unwrap_or(usize::MAX)),
    };
    if limits.disabled() {
        return messages;
    }
    let mut outcome = CompressionOutcome::default();
    for message in &mut messages {
        if let ModelMessage::Request(request) = message {
            for part in &mut request.parts {
                match part {
                    ModelRequestPart::UserPrompt { content, .. } => {
                        for item in content {
                            outcome.merge(compress_content_part(item, limits));
                        }
                    }
                    ModelRequestPart::ToolReturn(tool_return) => {
                        outcome.merge(compress_tool_value(&mut tool_return.content, limits));
                        if let Some(user_content) = &mut tool_return.user_content {
                            outcome.merge(compress_tool_value(user_content, limits));
                        }
                        if let Some(content_parts) = tool_return
                            .private_metadata
                            .get_mut("starweaver_tool_return_content_parts")
                        {
                            outcome.merge(compress_tool_value(content_parts, limits));
                        }
                    }
                    ModelRequestPart::SystemPrompt { .. }
                    | ModelRequestPart::RetryPrompt { .. }
                    | ModelRequestPart::Instruction { .. } => {}
                }
            }
        }
    }
    if outcome.compressed > 0 {
        request_metadata_mut(&mut messages).insert(
            "starweaver_media_compressed".to_string(),
            json!(outcome.compressed),
        );
    }
    if outcome.replaced > 0 {
        request_metadata_mut(&mut messages).insert(
            "starweaver_media_compression_replacements".to_string(),
            json!(outcome.replaced),
        );
    }
    if !outcome.failures.is_empty() {
        request_metadata_mut(&mut messages).insert(
            "starweaver_media_compression_failures".to_string(),
            Value::Array(outcome.failures),
        );
    }
    messages
}

fn compress_content_part(item: &mut ContentPart, limits: ImageLimits) -> CompressionOutcome {
    match item {
        ContentPart::Binary { .. } => compress_binary_content_part(item, limits),
        ContentPart::DataUrl { .. } => compress_data_url_content_part(item, limits),
        ContentPart::CachePoint { .. }
        | ContentPart::Text { .. }
        | ContentPart::ImageUrl { .. }
        | ContentPart::FileUrl { .. }
        | ContentPart::ResourceRef { .. } => CompressionOutcome::default(),
    }
}

fn compress_binary_content_part(item: &mut ContentPart, limits: ImageLimits) -> CompressionOutcome {
    let ContentPart::Binary { data, media_type } = item else {
        return CompressionOutcome::default();
    };
    if !is_inline_image(data, media_type)
        || !image_exceeds_model_limits(data, limits.max_encoded_bytes, limits.max_dimension)
    {
        return CompressionOutcome::default();
    }
    let original_size = data.len();
    match compress_image_to_model_limit(
        data,
        limits.max_encoded_bytes,
        limits.max_dimension,
        media_type,
    ) {
        Ok(compressed)
            if image_within_model_limits(
                &compressed.data,
                limits.max_encoded_bytes,
                limits.max_dimension,
            ) =>
        {
            *data = compressed.data;
            *media_type = compressed.media_type;
            CompressionOutcome::compressed(usize::from(compressed.compressed))
        }
        Ok(_) => {
            *item = ContentPart::Text {
                text: oversized_after_compression_message(
                    original_size,
                    limits.max_encoded_bytes,
                    limits.max_dimension,
                ),
            };
            CompressionOutcome::replaced(json!({
                "reason": "processed_image_exceeded_model_limit",
                "original_size": original_size,
                "max_image_bytes": limits.max_encoded_bytes,
                "max_image_dimension": limits.max_dimension,
            }))
        }
        Err(error) => {
            *item = ContentPart::Text {
                text: compression_failed_message(),
            };
            CompressionOutcome::replaced(json!({
                "reason": "image_processing_failed",
                "error": error,
            }))
        }
    }
}

fn compress_data_url_content_part(
    item: &mut ContentPart,
    limits: ImageLimits,
) -> CompressionOutcome {
    let ContentPart::DataUrl {
        data_url: content_data_url,
        media_type,
    } = item
    else {
        return CompressionOutcome::default();
    };
    let parsed = match parse_data_url(content_data_url) {
        Ok(parsed) => parsed,
        Err(error) => {
            *item = ContentPart::Text {
                text: compression_failed_message(),
            };
            return CompressionOutcome::replaced(json!({
                "reason": "invalid_data_url",
                "error": error,
            }));
        }
    };
    if !is_inline_image(&parsed.data, &parsed.media_type)
        || !image_exceeds_model_limits(&parsed.data, limits.max_encoded_bytes, limits.max_dimension)
    {
        return CompressionOutcome::default();
    }
    let original_size = parsed.data.len();
    match compress_image_to_model_limit(
        &parsed.data,
        limits.max_encoded_bytes,
        limits.max_dimension,
        &parsed.media_type,
    ) {
        Ok(compressed)
            if image_within_model_limits(
                &compressed.data,
                limits.max_encoded_bytes,
                limits.max_dimension,
            ) =>
        {
            *content_data_url = data_url(&compressed.media_type, &compressed.data);
            *media_type = compressed.media_type;
            CompressionOutcome::compressed(usize::from(compressed.compressed))
        }
        Ok(_) => {
            *item = ContentPart::Text {
                text: oversized_after_compression_message(
                    original_size,
                    limits.max_encoded_bytes,
                    limits.max_dimension,
                ),
            };
            CompressionOutcome::replaced(json!({
                "reason": "processed_data_url_exceeded_model_limit",
                "original_size": original_size,
                "max_image_bytes": limits.max_encoded_bytes,
                "max_image_dimension": limits.max_dimension,
            }))
        }
        Err(error) => {
            *item = ContentPart::Text {
                text: compression_failed_message(),
            };
            CompressionOutcome::replaced(json!({
                "reason": "data_url_image_processing_failed",
                "error": error,
            }))
        }
    }
}

fn compress_tool_value(value: &mut Value, limits: ImageLimits) -> CompressionOutcome {
    let mut outcome = CompressionOutcome::default();
    match value {
        Value::Array(items) => {
            for item in items {
                outcome.merge(compress_tool_value(item, limits));
            }
        }
        Value::Object(object) => {
            if object.get("data_url").and_then(Value::as_str).is_some() {
                match compress_data_url_object(
                    object,
                    limits.max_encoded_bytes,
                    limits.max_dimension,
                ) {
                    Ok(true) => outcome.compressed += 1,
                    Ok(false) => {}
                    Err(message) if message.starts_with("<system-reminder>") => {
                        *value = json!({ "type": "system_reminder", "text": message });
                        outcome.replaced += 1;
                    }
                    Err(error) => {
                        *value = json!({ "type": "system_reminder", "text": compression_failed_message() });
                        outcome.replaced += 1;
                        outcome.failures.push(json!({
                            "reason": "tool_data_url_compression_failed",
                            "error": error,
                        }));
                    }
                }
                return outcome;
            }
            if let Some(child_outcome) = compress_json_binary_content_object(object, limits) {
                outcome.merge(child_outcome);
                return outcome;
            }
            for item in object.values_mut() {
                outcome.merge(compress_tool_value(item, limits));
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
    outcome
}

fn compress_json_binary_content_object(
    object: &mut Map<String, Value>,
    limits: ImageLimits,
) -> Option<CompressionOutcome> {
    let media_type = object.get("media_type")?.as_str()?.to_string();
    let data = json_byte_array(object.get("data")?)?;
    if !is_inline_image(&data, &media_type) {
        return None;
    }
    if !image_exceeds_model_limits(&data, limits.max_encoded_bytes, limits.max_dimension) {
        return Some(CompressionOutcome::default());
    }
    let original_size = data.len();
    match compress_image_to_model_limit(
        &data,
        limits.max_encoded_bytes,
        limits.max_dimension,
        &media_type,
    ) {
        Ok(compressed)
            if image_within_model_limits(
                &compressed.data,
                limits.max_encoded_bytes,
                limits.max_dimension,
            ) =>
        {
            object.insert("data".to_string(), json!(compressed.data));
            object.insert("media_type".to_string(), json!(compressed.media_type));
            Some(CompressionOutcome::compressed(usize::from(
                compressed.compressed,
            )))
        }
        Ok(_) => {
            object.clear();
            object.insert("type".to_string(), json!("system_reminder"));
            object.insert(
                "text".to_string(),
                json!(oversized_after_compression_message(
                    original_size,
                    limits.max_encoded_bytes,
                    limits.max_dimension,
                )),
            );
            Some(CompressionOutcome::replaced(json!({
                "reason": "json_binary_image_exceeded_model_limit",
                "original_size": original_size,
                "max_image_bytes": limits.max_encoded_bytes,
                "max_image_dimension": limits.max_dimension,
            })))
        }
        Err(error) => {
            object.clear();
            object.insert("type".to_string(), json!("system_reminder"));
            object.insert("text".to_string(), json!(compression_failed_message()));
            Some(CompressionOutcome::replaced(json!({
                "reason": "json_binary_image_compression_failed",
                "error": error,
            })))
        }
    }
}

fn is_inline_image(data: &[u8], media_type: &str) -> bool {
    media_type.starts_with("image/") || detect_media_kind(data).is_image()
}

fn json_byte_array(value: &Value) -> Option<Vec<u8>> {
    value.as_array().map(|items| {
        items
            .iter()
            .map(|item| item.as_u64().and_then(|value| u8::try_from(value).ok()))
            .collect::<Option<Vec<_>>>()
    })?
}

#[derive(Clone, Copy)]
struct ImageLimits {
    max_encoded_bytes: usize,
    max_dimension: usize,
}

impl ImageLimits {
    const fn disabled(self) -> bool {
        self.max_encoded_bytes == 0 && self.max_dimension == 0
    }
}

#[derive(Default)]
struct CompressionOutcome {
    compressed: usize,
    replaced: usize,
    failures: Vec<Value>,
}

impl CompressionOutcome {
    const fn compressed(compressed: usize) -> Self {
        Self {
            compressed,
            replaced: 0,
            failures: Vec::new(),
        }
    }

    fn replaced(failure: Value) -> Self {
        Self {
            compressed: 0,
            replaced: 1,
            failures: vec![failure],
        }
    }

    fn merge(&mut self, other: Self) {
        self.compressed += other.compressed;
        self.replaced += other.replaced;
        self.failures.extend(other.failures);
    }
}
