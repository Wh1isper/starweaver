//! Media policy helpers.

use super::{MediaKind, MediaPolicy};

/// Return true when a media type represents an image.
#[must_use]
pub fn is_image_media_type(media_type: &str) -> bool {
    media_type.starts_with("image/")
}

/// Return true when a media type represents a video.
#[must_use]
pub fn is_video_media_type(media_type: &str) -> bool {
    media_type.starts_with("video/")
}

/// Return true when a media type is a document/file payload.
#[must_use]
pub fn is_document_media_type(media_type: &str) -> bool {
    !is_image_media_type(media_type) && !is_video_media_type(media_type)
}

pub(super) fn media_policy_reason(
    detected_kind: MediaKind,
    declared_media_type: Option<&str>,
    policy: &MediaPolicy,
) -> Option<String> {
    if detected_kind.is_image() || declared_media_type.is_some_and(is_image_media_type) {
        if !policy.allow_images {
            return Some("image media is disabled by policy".to_string());
        }
        if detected_kind == MediaKind::Gif && !policy.allow_gif {
            return Some("gif media is disabled by policy".to_string());
        }
        if detected_kind == MediaKind::Webp && !policy.allow_webp {
            return Some("webp media is disabled by policy".to_string());
        }
    }
    if (detected_kind.is_video() || declared_media_type.is_some_and(is_video_media_type))
        && !policy.allow_videos
    {
        return Some("video media is disabled by policy".to_string());
    }
    if detected_kind == MediaKind::Unknown
        && declared_media_type.is_some_and(is_document_media_type)
        && !policy.allow_documents
    {
        return Some("document media is disabled by policy".to_string());
    }
    None
}
