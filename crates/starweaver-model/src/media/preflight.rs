//! Media preflight inspection.

use serde::{Deserialize, Serialize};

use super::{
    ImageDimensions, MediaKind, MediaPolicy, base64_encoded_len, detect_image_dimensions,
    detect_media_kind, policy::media_policy_reason, raw_budget_from_base64_limit,
};

/// Media preflight result for a binary payload.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MediaPreflight {
    /// Detected kind.
    pub detected_kind: MediaKind,
    /// Declared input media type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declared_media_type: Option<String>,
    /// Corrected canonical media type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub corrected_media_type: Option<String>,
    /// Raw byte length.
    pub raw_bytes: usize,
    /// Base64 encoded byte length without line wrapping.
    pub base64_bytes: usize,
    /// True when declared and detected media types differ.
    pub media_type_corrected: bool,
    /// True when payload is an animated-capable media format.
    pub animated_capable: bool,
    /// Header-parsed image dimensions when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dimensions: Option<ImageDimensions>,
    /// True when payload shape is inconsistent with the detected media kind.
    pub corrupt: bool,
    /// Corruption explanation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub corruption_reason: Option<String>,
    /// True when inline base64 representation exceeds the configured budget.
    pub over_base64_budget: bool,
    /// True when image width or height exceeds the configured limit, or image
    /// dimensions cannot be validated while that limit is active.
    pub over_dimension_limit: bool,
    /// Raw byte budget derived from the base64 limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_raw_bytes: Option<usize>,
    /// True when policy permits this media payload.
    pub allowed_by_policy: bool,
    /// Policy rejection explanation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_reason: Option<String>,
}

impl MediaPreflight {
    /// Build preflight evidence for bytes and an optional declared media type.
    #[must_use]
    pub fn inspect(bytes: &[u8], declared_media_type: Option<&str>) -> Self {
        Self::inspect_with_policy(bytes, declared_media_type, &MediaPolicy::default())
    }

    /// Build preflight evidence with a policy budget and media capability filter.
    #[must_use]
    pub fn inspect_with_policy(
        bytes: &[u8],
        declared_media_type: Option<&str>,
        policy: &MediaPolicy,
    ) -> Self {
        let detected_kind = detect_media_kind(bytes);
        let detected_media_type = detected_kind.media_type();
        let corrected_media_type = detected_media_type
            .or(declared_media_type)
            .map(ToOwned::to_owned);
        let media_type_corrected = match (declared_media_type, detected_media_type) {
            (Some(declared), Some(detected)) => declared != detected,
            _ => false,
        };
        let dimensions = detect_image_dimensions(bytes, detected_kind);
        let corruption_reason = corruption_reason(bytes, detected_kind, dimensions);
        let base64_bytes = base64_encoded_len(bytes.len());
        let budget_raw_bytes = policy
            .max_inline_base64_bytes
            .filter(|limit| *limit > 0)
            .map(raw_budget_from_base64_limit);
        let over_base64_budget = policy
            .max_inline_base64_bytes
            .is_some_and(|limit| limit > 0 && base64_bytes > limit);
        let is_image =
            detected_kind.is_image() || declared_media_type.is_some_and(super::is_image_media_type);
        let over_dimension_limit = policy.max_image_dimension.is_some_and(|limit| {
            limit > 0
                && is_image
                && dimensions
                    .is_none_or(|dimensions| dimensions.width > limit || dimensions.height > limit)
        });
        let policy_reason = media_policy_reason(detected_kind, declared_media_type, policy);
        Self {
            detected_kind,
            declared_media_type: declared_media_type.map(ToOwned::to_owned),
            corrected_media_type,
            raw_bytes: bytes.len(),
            base64_bytes,
            media_type_corrected,
            animated_capable: detected_kind.can_be_animated(),
            dimensions,
            corrupt: corruption_reason.is_some(),
            corruption_reason,
            over_base64_budget,
            over_dimension_limit,
            budget_raw_bytes,
            allowed_by_policy: policy_reason.is_none(),
            policy_reason,
        }
    }
}

fn corruption_reason(
    bytes: &[u8],
    kind: MediaKind,
    dimensions: Option<ImageDimensions>,
) -> Option<String> {
    match kind {
        MediaKind::Png => {
            if bytes.len() < 33 || &bytes[12..16] != b"IHDR" {
                Some("png payload is missing an IHDR header".to_string())
            } else if dimensions.is_none() {
                Some("png dimensions could not be parsed".to_string())
            } else {
                None
            }
        }
        MediaKind::Jpeg => {
            if bytes.len() < 4 || !bytes.ends_with(b"\xff\xd9") {
                Some("jpeg payload is missing an end marker".to_string())
            } else if dimensions.is_none() {
                Some("jpeg dimensions could not be parsed".to_string())
            } else {
                None
            }
        }
        MediaKind::Gif => {
            if bytes.len() < 13 || dimensions.is_none() {
                Some("gif payload is missing a logical screen descriptor".to_string())
            } else {
                None
            }
        }
        MediaKind::Webp => {
            if bytes.len() < 16 || dimensions.is_none() {
                Some("webp payload is missing a supported image header".to_string())
            } else {
                None
            }
        }
        MediaKind::Mp4 => {
            if bytes.len() < 12 {
                Some("mp4 payload is missing an ftyp box".to_string())
            } else {
                None
            }
        }
        MediaKind::Webm => {
            if bytes.len() < 4 {
                Some("webm payload is missing an EBML header".to_string())
            } else {
                None
            }
        }
        MediaKind::Unknown => {
            if bytes.is_empty() {
                Some("media payload is empty".to_string())
            } else {
                None
            }
        }
    }
}
