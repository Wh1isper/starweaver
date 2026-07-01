//! Media compression filtering.

use serde_json::{Map, Value, json};
use starweaver_context::AgentContext;
use starweaver_model::{ContentPart, ModelMessage, ModelRequestPart, parse_data_url};
use starweaver_runtime::AgentRunState;

use crate::{
    filters::message::request_metadata_mut,
    media_compression::{
        compress_data_url_object, compress_image_data, compression_failed_message, data_url,
        oversized_after_compression_message, raw_budget_for_encoded_limit,
    },
};

use super::policy::media_policy_from_state_and_context;

pub(in crate::filters) fn media_compress_filter(
    state: &AgentRunState,
    context: &AgentContext,
    mut messages: Vec<ModelMessage>,
) -> Vec<ModelMessage> {
    let policy = media_policy_from_state_and_context(state, context);
    let Some(max_image_bytes) = policy.max_inline_base64_bytes else {
        return messages;
    };
    if max_image_bytes == 0 {
        return messages;
    }
    let mut outcome = CompressionOutcome::default();
    for message in &mut messages {
        if let ModelMessage::Request(request) = message {
            for part in &mut request.parts {
                match part {
                    ModelRequestPart::UserPrompt { content, .. } => {
                        for item in content {
                            outcome.merge(compress_content_part(item, max_image_bytes));
                        }
                    }
                    ModelRequestPart::ToolReturn(tool_return) => {
                        outcome.merge(compress_tool_value(
                            &mut tool_return.content,
                            max_image_bytes,
                        ));
                        if let Some(user_content) = &mut tool_return.user_content {
                            outcome.merge(compress_tool_value(user_content, max_image_bytes));
                        }
                        if let Some(content_parts) = tool_return
                            .private_metadata
                            .get_mut("starweaver_tool_return_content_parts")
                        {
                            outcome.merge(compress_tool_value(content_parts, max_image_bytes));
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

fn compress_content_part(item: &mut ContentPart, max_image_bytes: usize) -> CompressionOutcome {
    let max_raw_bytes = raw_budget_for_encoded_limit(max_image_bytes);
    match item {
        ContentPart::Binary { data, media_type } if media_type.starts_with("image/") => {
            if data.len() <= max_raw_bytes {
                return CompressionOutcome::default();
            }
            let original_size = data.len();
            match compress_image_data(data, max_raw_bytes, media_type) {
                Ok(compressed) if compressed.data.len() <= max_raw_bytes => {
                    *data = compressed.data;
                    *media_type = compressed.media_type;
                    CompressionOutcome::compressed(usize::from(compressed.compressed))
                }
                Ok(_) => {
                    *item = ContentPart::Text {
                        text: oversized_after_compression_message(original_size, max_image_bytes),
                    };
                    CompressionOutcome::replaced(json!({
                        "reason": "compressed_image_exceeded_model_limit",
                        "original_size": original_size,
                        "max_image_bytes": max_image_bytes,
                    }))
                }
                Err(error) => {
                    *item = ContentPart::Text {
                        text: compression_failed_message(),
                    };
                    CompressionOutcome::replaced(json!({
                        "reason": "compression_failed",
                        "error": error,
                    }))
                }
            }
        }
        ContentPart::DataUrl {
            data_url: content_data_url,
            media_type,
        } if media_type.starts_with("image/") => match parse_data_url(content_data_url) {
            Ok(parsed) => {
                if parsed.data.len() <= max_raw_bytes {
                    return CompressionOutcome::default();
                }
                let original_size = parsed.data.len();
                match compress_image_data(&parsed.data, max_raw_bytes, &parsed.media_type) {
                    Ok(compressed) if compressed.data.len() <= max_raw_bytes => {
                        *content_data_url = data_url(&compressed.media_type, &compressed.data);
                        *media_type = compressed.media_type;
                        CompressionOutcome::compressed(usize::from(compressed.compressed))
                    }
                    Ok(_) => {
                        *item = ContentPart::Text {
                            text: oversized_after_compression_message(
                                original_size,
                                max_image_bytes,
                            ),
                        };
                        CompressionOutcome::replaced(json!({
                            "reason": "compressed_data_url_exceeded_model_limit",
                            "original_size": original_size,
                            "max_image_bytes": max_image_bytes,
                        }))
                    }
                    Err(error) => {
                        *item = ContentPart::Text {
                            text: compression_failed_message(),
                        };
                        CompressionOutcome::replaced(json!({
                            "reason": "data_url_compression_failed",
                            "error": error,
                        }))
                    }
                }
            }
            Err(error) => CompressionOutcome::failure(json!({
                "reason": "invalid_data_url",
                "error": error,
            })),
        },
        ContentPart::Text { .. }
        | ContentPart::ImageUrl { .. }
        | ContentPart::FileUrl { .. }
        | ContentPart::ResourceRef { .. }
        | ContentPart::Binary { .. }
        | ContentPart::DataUrl { .. } => CompressionOutcome::default(),
    }
}

fn compress_tool_value(value: &mut Value, max_image_bytes: usize) -> CompressionOutcome {
    let mut outcome = CompressionOutcome::default();
    match value {
        Value::Array(items) => {
            for item in items {
                outcome.merge(compress_tool_value(item, max_image_bytes));
            }
        }
        Value::Object(object) => {
            if object.get("data_url").and_then(Value::as_str).is_some() {
                match compress_data_url_object(object, max_image_bytes) {
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
            if let Some(child_outcome) =
                compress_json_binary_content_object(object, max_image_bytes)
            {
                outcome.merge(child_outcome);
                return outcome;
            }
            for item in object.values_mut() {
                outcome.merge(compress_tool_value(item, max_image_bytes));
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
    outcome
}

fn compress_json_binary_content_object(
    object: &mut Map<String, Value>,
    max_image_bytes: usize,
) -> Option<CompressionOutcome> {
    let media_type = object.get("media_type")?.as_str()?.to_string();
    if !media_type.starts_with("image/") {
        return None;
    }
    let data = json_byte_array(object.get("data")?)?;
    let max_raw_bytes = raw_budget_for_encoded_limit(max_image_bytes);
    if data.len() <= max_raw_bytes {
        return Some(CompressionOutcome::default());
    }
    let original_size = data.len();
    match compress_image_data(&data, max_raw_bytes, &media_type) {
        Ok(compressed) if compressed.data.len() <= max_raw_bytes => {
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
                    max_image_bytes,
                )),
            );
            Some(CompressionOutcome::replaced(json!({
                "reason": "json_binary_image_exceeded_model_limit",
                "original_size": original_size,
                "max_image_bytes": max_image_bytes,
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

fn json_byte_array(value: &Value) -> Option<Vec<u8>> {
    value.as_array().map(|items| {
        items
            .iter()
            .map(|item| item.as_u64().and_then(|value| u8::try_from(value).ok()))
            .collect::<Option<Vec<_>>>()
    })?
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

    fn failure(failure: Value) -> Self {
        Self {
            compressed: 0,
            replaced: 0,
            failures: vec![failure],
        }
    }

    fn merge(&mut self, other: Self) {
        self.compressed += other.compressed;
        self.replaced += other.replaced;
        self.failures.extend(other.failures);
    }
}
