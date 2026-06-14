//! Media filter policy helpers.

use starweaver_context::AgentContext;
use starweaver_model::{ContentPart, MediaPolicy};
use starweaver_runtime::AgentRunState;

const MEDIA_POLICY_METADATA: &str = "starweaver_media_policy";

pub(super) fn media_policy_from_state_and_context(
    state: &AgentRunState,
    context: &AgentContext,
) -> MediaPolicy {
    let mut policy: MediaPolicy = state
        .metadata
        .get(MEDIA_POLICY_METADATA)
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default();
    if policy.max_inline_base64_bytes.is_none() && context.model_config.max_image_bytes > 0 {
        policy.max_inline_base64_bytes = Some(context.model_config.max_image_bytes);
    }
    if policy.max_images.is_none() {
        policy.max_images = Some(context.model_config.max_images);
    }
    if policy.max_videos.is_none() {
        policy.max_videos = Some(context.model_config.max_videos);
    }
    policy.allow_gif &= context.model_config.support_gif;
    policy
}

pub(super) fn is_image_content(item: &ContentPart) -> bool {
    match item {
        ContentPart::ImageUrl { .. } => true,
        ContentPart::Binary { media_type, .. }
        | ContentPart::DataUrl { media_type, .. }
        | ContentPart::FileUrl { media_type, .. }
        | ContentPart::ResourceRef { media_type, .. } => media_type.starts_with("image/"),
        ContentPart::Text { .. } => false,
    }
}

pub(super) fn is_video_content(item: &ContentPart) -> bool {
    match item {
        ContentPart::Binary { media_type, .. }
        | ContentPart::DataUrl { media_type, .. }
        | ContentPart::FileUrl { media_type, .. }
        | ContentPart::ResourceRef { media_type, .. } => media_type.starts_with("video/"),
        ContentPart::ImageUrl { .. } | ContentPart::Text { .. } => false,
    }
}

pub(super) fn content_policy_reason(item: &ContentPart, policy: &MediaPolicy) -> Option<String> {
    match item {
        ContentPart::ImageUrl { .. } if !policy.allow_images => {
            Some("image media is disabled".to_string())
        }
        ContentPart::FileUrl { media_type, .. }
        | ContentPart::ResourceRef { media_type, .. }
        | ContentPart::DataUrl { media_type, .. }
        | ContentPart::Binary { media_type, .. } => {
            if media_type.starts_with("image/") && !policy.allow_images {
                Some("image media is disabled".to_string())
            } else if media_type.starts_with("video/") && !policy.allow_videos {
                Some("video media is disabled".to_string())
            } else if !media_type.starts_with("image/")
                && !media_type.starts_with("video/")
                && !policy.allow_documents
            {
                Some("document media is disabled".to_string())
            } else if media_type == "image/gif" && !policy.allow_gif {
                Some("gif media is disabled".to_string())
            } else if media_type == "image/webp" && !policy.allow_webp {
                Some("webp media is disabled".to_string())
            } else {
                None
            }
        }
        ContentPart::Text { .. } | ContentPart::ImageUrl { .. } => None,
    }
}
