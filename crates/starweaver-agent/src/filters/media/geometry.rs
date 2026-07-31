//! Admission and bounded retention for immutable geometry-bound media.

use std::collections::BTreeSet;

use serde_json::Value;
use starweaver_context::{AgentContext, ModelCapability};
use starweaver_model::{
    ContentPart, MediaPreflight, ModelMessage, ModelRequestPart, parse_data_url,
};
use starweaver_runtime::AgentRunState;

use super::policy::geometry_bound_media;

const GEOMETRY_BOUND_MEDIA_METADATA: &str = "starweaver_geometry_bound_immutable_media";
const TOOL_RETURN_CONTENT_PARTS_METADATA: &str = "starweaver_tool_return_content_parts";
const TOOL_RETURN_PROMPT_METADATA: &str = "starweaver_tool_return_prompt";

/// Prune stale Computer Use screenshots and admit the exact retained bytes for the active model.
///
/// Geometry-bound images cannot be resized, recompressed, split, or replaced after observation
/// creation. This processor therefore removes whole stale media prompts (and their private tool
/// payloads) before applying hard model limits to the newest retained observation set.
pub(in crate::filters) fn geometry_media_admission_filter(
    state: &mut AgentRunState,
    context: &mut AgentContext,
    mut messages: Vec<ModelMessage>,
) -> Result<Vec<ModelMessage>, String> {
    let candidates = geometry_candidates(&messages)?;
    if candidates.is_empty() {
        return Ok(messages);
    }

    let retained = retained_candidates(&candidates, context);
    let stale_keys = candidates
        .iter()
        .filter(|candidate| !retained.contains(&(candidate.message_index, candidate.part_index)))
        .map(|candidate| (candidate.message_index, candidate.part_index))
        .collect::<BTreeSet<_>>();
    let stale_tool_call_ids = candidates
        .iter()
        .filter(|candidate| stale_keys.contains(&(candidate.message_index, candidate.part_index)))
        .filter_map(|candidate| candidate.tool_call_id.clone())
        .collect::<BTreeSet<_>>();

    remove_stale_geometry_media(&mut messages, &stale_keys, &stale_tool_call_ids);
    if messages != state.message_history {
        state.message_history.clone_from(&messages);
        context.message_history.clone_from(&messages);
    }

    admit_retained_geometry_media(&messages, context)?;
    Ok(messages)
}

#[derive(Clone, Debug)]
struct GeometryCandidate {
    message_index: usize,
    part_index: usize,
    tool_call_id: Option<String>,
    image_count: usize,
    encoded_bytes: usize,
}

fn geometry_candidates(messages: &[ModelMessage]) -> Result<Vec<GeometryCandidate>, String> {
    let mut candidates = Vec::new();
    for (message_index, message) in messages.iter().enumerate() {
        let ModelMessage::Request(request) = message else {
            continue;
        };
        for (part_index, part) in request.parts.iter().enumerate() {
            let ModelRequestPart::UserPrompt {
                content, metadata, ..
            } = part
            else {
                continue;
            };
            if !geometry_bound_media(metadata) {
                continue;
            }
            let (image_count, encoded_bytes) = geometry_content_stats(content)?;
            if image_count == 0 {
                return Err(safety_error(
                    "a geometry-bound media prompt contains no exact inline image",
                ));
            }
            candidates.push(GeometryCandidate {
                message_index,
                part_index,
                tool_call_id: metadata
                    .get("tool_call_id")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                image_count,
                encoded_bytes,
            });
        }
    }
    Ok(candidates)
}

fn geometry_content_stats(content: &[ContentPart]) -> Result<(usize, usize), String> {
    let mut image_count = 0usize;
    let mut encoded_bytes = 0usize;
    for part in content {
        let (bytes, media_type) = match part {
            ContentPart::Binary { data, media_type } if media_type.starts_with("image/") => {
                (data.as_slice(), media_type.as_str())
            }
            ContentPart::DataUrl {
                data_url,
                media_type,
            } if media_type.starts_with("image/") => {
                let parsed = parse_data_url(data_url).map_err(|error| {
                    safety_error(format!(
                        "a geometry-bound image data URL is invalid: {error}"
                    ))
                })?;
                if !parsed.media_type.starts_with("image/") {
                    return Err(safety_error(
                        "a geometry-bound data URL does not contain image media",
                    ));
                }
                image_count = image_count.saturating_add(1);
                encoded_bytes = encoded_bytes
                    .checked_add(starweaver_model::base64_encoded_len(parsed.data.len()))
                    .ok_or_else(|| {
                        safety_error("geometry-bound image byte accounting overflowed")
                    })?;
                let preflight = MediaPreflight::inspect(&parsed.data, Some(media_type));
                validate_exact_image(&preflight)?;
                continue;
            }
            ContentPart::CachePoint { .. } | ContentPart::Text { .. } => continue,
            ContentPart::ImageUrl { .. }
            | ContentPart::FileUrl { .. }
            | ContentPart::ResourceRef { .. }
            | ContentPart::Binary { .. }
            | ContentPart::DataUrl { .. } => {
                return Err(safety_error(
                    "geometry-bound media must be an exact inline image payload",
                ));
            }
        };
        image_count = image_count.saturating_add(1);
        encoded_bytes = encoded_bytes
            .checked_add(starweaver_model::base64_encoded_len(bytes.len()))
            .ok_or_else(|| safety_error("geometry-bound image byte accounting overflowed"))?;
        let preflight = MediaPreflight::inspect(bytes, Some(media_type));
        validate_exact_image(&preflight)?;
    }
    Ok((image_count, encoded_bytes))
}

fn validate_exact_image(preflight: &MediaPreflight) -> Result<(), String> {
    if preflight.corrupt || !preflight.detected_kind.is_image() {
        return Err(safety_error(
            preflight
                .corruption_reason
                .as_deref()
                .unwrap_or("geometry-bound payload is not a supported, structurally valid image"),
        ));
    }
    if preflight.media_type_corrected {
        return Err(safety_error(
            "geometry-bound image MIME type does not match its exact bytes",
        ));
    }
    Ok(())
}

fn retained_candidates(
    candidates: &[GeometryCandidate],
    context: &AgentContext,
) -> BTreeSet<(usize, usize)> {
    let max_images = context.model_config.max_images;
    let max_total_bytes = context.model_config.max_image_bytes;
    let mut retained = BTreeSet::new();
    let mut retained_images = 0usize;
    let mut retained_bytes = 0usize;
    let mut tail_has_capacity = true;

    for (reverse_index, candidate) in candidates.iter().rev().enumerate() {
        let next_images = retained_images.saturating_add(candidate.image_count);
        let next_bytes = retained_bytes.saturating_add(candidate.encoded_bytes);
        // Always preserve the newest basis. Admission below rejects it explicitly when the
        // active model cannot consume it; silently deleting the current basis would be unsafe.
        let fits = reverse_index == 0
            || (tail_has_capacity
                && next_images <= max_images
                && (max_total_bytes == 0 || next_bytes <= max_total_bytes));
        if fits {
            retained.insert((candidate.message_index, candidate.part_index));
            retained_images = next_images;
            retained_bytes = next_bytes;
        } else {
            // Retention is one contiguous newest-first tail. Once a newer candidate crosses a
            // hard bound, every older candidate is stale even if it happens to be smaller.
            tail_has_capacity = false;
        }
    }
    retained
}

fn remove_stale_geometry_media(
    messages: &mut Vec<ModelMessage>,
    stale_keys: &BTreeSet<(usize, usize)>,
    stale_tool_call_ids: &BTreeSet<String>,
) {
    for (message_index, message) in messages.iter_mut().enumerate() {
        let ModelMessage::Request(request) = message else {
            continue;
        };
        let mut original_index = 0usize;
        request.parts.retain_mut(|part| {
            let remove = stale_keys.contains(&(message_index, original_index));
            original_index = original_index.saturating_add(1);
            if remove {
                return false;
            }
            if let ModelRequestPart::ToolReturn(tool_return) = part
                && stale_tool_call_ids.contains(&tool_return.tool_call_id)
                && tool_return
                    .private_metadata
                    .get(GEOMETRY_BOUND_MEDIA_METADATA)
                    .and_then(Value::as_bool)
                    == Some(true)
            {
                tool_return
                    .private_metadata
                    .remove(TOOL_RETURN_CONTENT_PARTS_METADATA);
                tool_return
                    .private_metadata
                    .remove(TOOL_RETURN_PROMPT_METADATA);
                tool_return
                    .private_metadata
                    .remove(GEOMETRY_BOUND_MEDIA_METADATA);
            }
            true
        });
    }
    messages.retain(
        |message| !matches!(message, ModelMessage::Request(request) if request.parts.is_empty()),
    );
}

fn admit_retained_geometry_media(
    messages: &[ModelMessage],
    context: &AgentContext,
) -> Result<(), String> {
    let candidates = geometry_candidates(messages)?;
    if candidates.is_empty() {
        return Ok(());
    }
    if !context
        .model_config
        .capabilities
        .contains(&ModelCapability::Vision)
    {
        return Err(safety_error(
            "the active model does not advertise image capability",
        ));
    }

    let image_count = candidates.iter().try_fold(0usize, |total, candidate| {
        total
            .checked_add(candidate.image_count)
            .ok_or_else(|| safety_error("geometry-bound image count overflowed"))
    })?;
    if image_count > context.model_config.max_images {
        return Err(safety_error(format!(
            "retained geometry-bound image count {image_count} exceeds active model max_images={}",
            context.model_config.max_images
        )));
    }

    let max_image_bytes = context.model_config.max_image_bytes;
    if max_image_bytes > 0 {
        if let Some(candidate) = candidates
            .iter()
            .find(|candidate| candidate.encoded_bytes > max_image_bytes)
        {
            return Err(safety_error(format!(
                "one geometry-bound media prompt requires {} base64 bytes, exceeding active model max_image_bytes={max_image_bytes}",
                candidate.encoded_bytes
            )));
        }
        let total_bytes = candidates.iter().try_fold(0usize, |total, candidate| {
            total
                .checked_add(candidate.encoded_bytes)
                .ok_or_else(|| safety_error("geometry-bound image byte accounting overflowed"))
        })?;
        if total_bytes > max_image_bytes {
            return Err(safety_error(format!(
                "retained geometry-bound images require {total_bytes} total base64 bytes, exceeding active model hard aggregate limit max_image_bytes={max_image_bytes}"
            )));
        }
    }

    let max_dimension = context.model_config.max_image_dimension;
    if max_dimension > 0 {
        for message in messages {
            let ModelMessage::Request(request) = message else {
                continue;
            };
            for part in &request.parts {
                let ModelRequestPart::UserPrompt {
                    content, metadata, ..
                } = part
                else {
                    continue;
                };
                if !geometry_bound_media(metadata) {
                    continue;
                }
                for content_part in content {
                    let preflight = match content_part {
                        ContentPart::Binary { data, media_type } => {
                            Some(MediaPreflight::inspect(data, Some(media_type)))
                        }
                        ContentPart::DataUrl {
                            data_url,
                            media_type,
                        } => parse_data_url(data_url)
                            .ok()
                            .map(|parsed| MediaPreflight::inspect(&parsed.data, Some(media_type))),
                        ContentPart::CachePoint { .. }
                        | ContentPart::Text { .. }
                        | ContentPart::ImageUrl { .. }
                        | ContentPart::FileUrl { .. }
                        | ContentPart::ResourceRef { .. } => None,
                    };
                    if let Some(preflight) = preflight
                        && preflight.dimensions.is_none_or(|dimensions| {
                            usize::try_from(dimensions.width).unwrap_or(usize::MAX) > max_dimension
                                || usize::try_from(dimensions.height).unwrap_or(usize::MAX)
                                    > max_dimension
                        })
                    {
                        return Err(safety_error(format!(
                            "a geometry-bound image exceeds active model max_image_dimension={max_dimension}"
                        )));
                    }
                }
            }
        }
    }
    Ok(())
}

fn safety_error(detail: impl AsRef<str>) -> String {
    format!(
        "Computer Use safety admission rejected exact geometry-bound media: {}. The screenshot was not transformed or submitted to the model",
        detail.as_ref()
    )
}

#[cfg(test)]
mod tests {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use serde_json::json;
    use starweaver_context::{AgentContext, ModelCapability};
    use starweaver_core::{ConversationId, RunId};
    use starweaver_model::{
        ContentPart, ModelMessage, ModelRequest, ModelRequestPart, ToolReturnPart, parse_data_url,
    };
    use starweaver_runtime::AgentRunState;

    use super::{
        GEOMETRY_BOUND_MEDIA_METADATA, TOOL_RETURN_CONTENT_PARTS_METADATA,
        TOOL_RETURN_PROMPT_METADATA, geometry_media_admission_filter,
    };

    fn png_bytes(marker: u8) -> Vec<u8> {
        let mut bytes = vec![0_u8; 33];
        bytes[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        bytes[8..12].copy_from_slice(&13_u32.to_be_bytes());
        bytes[12..16].copy_from_slice(b"IHDR");
        bytes[16..20].copy_from_slice(&1_u32.to_be_bytes());
        bytes[20..24].copy_from_slice(&1_u32.to_be_bytes());
        bytes[24] = marker;
        bytes
    }

    fn geometry_turn(call_id: &str, marker: u8) -> ModelMessage {
        let bytes = png_bytes(marker);
        let data_url = format!("data:image/png;base64,{}", STANDARD.encode(&bytes));
        let private_metadata = serde_json::Map::from_iter([
            (
                TOOL_RETURN_CONTENT_PARTS_METADATA.to_owned(),
                json!([{
                    "kind": "data_url",
                    "data_url": data_url,
                    "media_type": "image/png"
                }]),
            ),
            (
                TOOL_RETURN_PROMPT_METADATA.to_owned(),
                json!(format!("observation {call_id}")),
            ),
            (GEOMETRY_BOUND_MEDIA_METADATA.to_owned(), json!(true)),
        ]);
        ModelMessage::Request(ModelRequest {
            parts: vec![
                ModelRequestPart::ToolReturn(
                    ToolReturnPart::new(call_id, "computer_observe", json!({"ok": true}))
                        .with_private_metadata(private_metadata),
                ),
                ModelRequestPart::UserPrompt {
                    content: vec![
                        ContentPart::Text {
                            text: format!("observation {call_id}"),
                        },
                        ContentPart::DataUrl {
                            data_url: format!("data:image/png;base64,{}", STANDARD.encode(&bytes)),
                            media_type: "image/png".to_owned(),
                        },
                    ],
                    name: None,
                    metadata: serde_json::Map::from_iter([
                        (GEOMETRY_BOUND_MEDIA_METADATA.to_owned(), json!(true)),
                        ("tool_call_id".to_owned(), json!(call_id)),
                    ]),
                },
            ],
            timestamp: None,
            instructions: None,
            run_id: None,
            conversation_id: None,
            metadata: serde_json::Map::new(),
        })
    }

    fn state_with(messages: Vec<ModelMessage>) -> (AgentRunState, AgentContext) {
        let mut state = AgentRunState::new(
            RunId::from_string("run-geometry-admission"),
            ConversationId::from_string("conversation-geometry-admission"),
        );
        state.message_history = messages.clone();
        let mut context = AgentContext {
            message_history: messages,
            ..AgentContext::default()
        };
        context
            .model_config
            .capabilities
            .insert(ModelCapability::Vision);
        context.model_config.max_image_dimension = 64;
        (state, context)
    }

    fn geometry_payloads(messages: &[ModelMessage]) -> Vec<Vec<u8>> {
        messages
            .iter()
            .filter_map(|message| match message {
                ModelMessage::Request(request) => Some(&request.parts),
                ModelMessage::Response(_) => None,
            })
            .flatten()
            .filter_map(|part| match part {
                ModelRequestPart::UserPrompt {
                    content, metadata, ..
                } if metadata
                    .get(GEOMETRY_BOUND_MEDIA_METADATA)
                    .and_then(serde_json::Value::as_bool)
                    == Some(true) =>
                {
                    Some(content)
                }
                _ => None,
            })
            .flatten()
            .filter_map(|part| match part {
                ContentPart::DataUrl { data_url, .. } => {
                    parse_data_url(data_url).ok().map(|parsed| parsed.data)
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn retention_removes_whole_stale_media_and_preserves_newest_exact_bytes() {
        let original = vec![
            geometry_turn("call-1", 1),
            geometry_turn("call-2", 2),
            geometry_turn("call-3", 3),
        ];
        let expected = vec![png_bytes(2), png_bytes(3)];
        let (mut state, mut context) = state_with(original);
        context.model_config.max_images = 2;
        context.model_config.max_image_bytes = 1_000;

        let input = state.message_history.clone();
        let Ok(filtered) = geometry_media_admission_filter(&mut state, &mut context, input) else {
            panic!("valid retained geometry media should be admitted");
        };

        assert_eq!(geometry_payloads(&filtered), expected);
        let ModelMessage::Request(oldest) = &filtered[0] else {
            panic!("oldest turn should remain as a structured tool return");
        };
        assert_eq!(
            oldest.parts.len(),
            1,
            "the stale media prompt is removed whole"
        );
        let ModelRequestPart::ToolReturn(oldest_return) = &oldest.parts[0] else {
            panic!("structured tool return should remain");
        };
        assert!(
            !oldest_return
                .private_metadata
                .contains_key(TOOL_RETURN_CONTENT_PARTS_METADATA)
        );
        assert!(
            !oldest_return
                .private_metadata
                .contains_key(TOOL_RETURN_PROMPT_METADATA)
        );
        assert!(
            !oldest_return
                .private_metadata
                .contains_key(GEOMETRY_BOUND_MEDIA_METADATA)
        );
        assert_eq!(state.message_history, filtered);
        assert_eq!(context.message_history, filtered);
    }

    #[test]
    fn aggregate_byte_retention_drops_old_media_without_reencoding_newest() {
        let original = vec![
            geometry_turn("call-1", 1),
            geometry_turn("call-2", 2),
            geometry_turn("call-3", 3),
        ];
        let exact_encoded_bytes = starweaver_model::base64_encoded_len(png_bytes(0).len());
        let (mut state, mut context) = state_with(original);
        context.model_config.max_images = 3;
        context.model_config.max_image_bytes = exact_encoded_bytes * 2;

        let input = state.message_history.clone();
        let Ok(filtered) = geometry_media_admission_filter(&mut state, &mut context, input) else {
            panic!("valid newest geometry media should be admitted");
        };
        assert_eq!(
            geometry_payloads(&filtered),
            vec![png_bytes(2), png_bytes(3)]
        );
    }

    #[test]
    fn active_model_switch_without_vision_rejects_before_submission() {
        let (mut state, mut context) = state_with(vec![geometry_turn("call-1", 1)]);
        context
            .model_config
            .capabilities
            .remove(&ModelCapability::Vision);

        let input = state.message_history.clone();
        let Err(error) = geometry_media_admission_filter(&mut state, &mut context, input) else {
            panic!("a model switch without image capability must reject retained media");
        };
        assert!(error.contains("safety admission rejected"));
        assert!(error.contains("does not advertise image capability"));
        assert!(error.contains("not transformed or submitted"));
    }

    #[test]
    fn active_model_zero_count_and_single_byte_limits_reject_current_basis() {
        let (mut count_state, mut count_context) = state_with(vec![geometry_turn("call-1", 1)]);
        count_context.model_config.max_images = 0;
        let count_input = count_state.message_history.clone();
        let Err(count_error) =
            geometry_media_admission_filter(&mut count_state, &mut count_context, count_input)
        else {
            panic!("a zero count limit must reject the newest observation basis");
        };
        assert!(count_error.contains("max_images=0"));

        let (mut byte_state, mut byte_context) = state_with(vec![geometry_turn("call-2", 2)]);
        byte_context.model_config.max_image_bytes = 1;
        let byte_input = byte_state.message_history.clone();
        let Err(byte_error) =
            geometry_media_admission_filter(&mut byte_state, &mut byte_context, byte_input)
        else {
            panic!("an undersized byte limit must reject the newest observation basis");
        };
        assert!(byte_error.contains("one geometry-bound media prompt requires"));
        assert!(byte_error.contains("max_image_bytes=1"));
    }
}
