//! Media splitting filter.

use serde_json::{Value, json};
use starweaver_context::AgentContext;
use starweaver_model::{ContentPart, ModelMessage, ModelRequestPart, detect_media_kind};
use starweaver_runtime::AgentRunState;

use crate::{
    filters::message::request_metadata_mut,
    media_compression::{ImageSegment, split_image_data},
};

pub(in crate::filters) fn media_split_filter(
    _state: &AgentRunState,
    context: &AgentContext,
    mut messages: Vec<ModelMessage>,
) -> Vec<ModelMessage> {
    if !context.model_config.split_large_images || context.model_config.image_split_max_height == 0
    {
        return messages;
    }

    let mut split_images = 0usize;
    let mut split_segments = 0usize;
    let mut failures = Vec::new();
    for message in &mut messages {
        if let ModelMessage::Request(request) = message {
            for part in &mut request.parts {
                let ModelRequestPart::UserPrompt { content, .. } = part else {
                    continue;
                };
                let outcome = split_content_parts(
                    content,
                    context.model_config.image_split_max_height,
                    context.model_config.image_split_overlap,
                );
                split_images += outcome.split_images;
                split_segments += outcome.split_segments;
                failures.extend(outcome.failures);
            }
        }
    }
    if split_images > 0 {
        request_metadata_mut(&mut messages).insert(
            "starweaver_media_split".to_string(),
            json!({
                "images": split_images,
                "segments": split_segments,
            }),
        );
    }
    if !failures.is_empty() {
        request_metadata_mut(&mut messages).insert(
            "starweaver_media_split_failures".to_string(),
            Value::Array(failures),
        );
    }
    messages
}

fn split_content_parts(
    content: &mut Vec<ContentPart>,
    max_height: usize,
    overlap: usize,
) -> SplitOutcome {
    let mut outcome = SplitOutcome::default();
    let mut processed = Vec::with_capacity(content.len());
    for item in std::mem::take(content) {
        match item {
            ContentPart::Binary { data, media_type }
                if media_type.starts_with("image/") || detect_media_kind(&data).is_image() =>
            {
                match split_image_data(&data, max_height, overlap, &media_type) {
                    Ok(segments) if segments.len() > 1 => {
                        outcome.split_images += 1;
                        outcome.split_segments += segments.len();
                        processed.extend(segments.into_iter().map(content_part_from_segment));
                    }
                    Ok(mut segments) if segments.len() == 1 => {
                        let segment = segments.remove(0);
                        processed.push(content_part_from_segment(segment));
                    }
                    Ok(_) => processed.push(ContentPart::Binary { data, media_type }),
                    Err(error) => {
                        outcome.failures.push(json!({
                            "reason": "image_split_failed",
                            "error": error,
                            "media_type": media_type,
                        }));
                        processed.push(ContentPart::Binary { data, media_type });
                    }
                }
            }
            other => processed.push(other),
        }
    }
    *content = processed;
    outcome
}

fn content_part_from_segment(segment: ImageSegment) -> ContentPart {
    ContentPart::Binary {
        data: segment.data,
        media_type: segment.media_type,
    }
}

#[derive(Default)]
struct SplitOutcome {
    split_images: usize,
    split_segments: usize,
    failures: Vec<Value>,
}
