//! Media preflight helpers for provider-neutral model content.

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Deserialize, Serialize};

/// Detected media kind.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    /// PNG image.
    Png,
    /// JPEG image.
    Jpeg,
    /// GIF image.
    Gif,
    /// WebP image.
    Webp,
    /// MP4 video.
    Mp4,
    /// `WebM` video.
    Webm,
    /// Unknown binary payload.
    Unknown,
}

impl MediaKind {
    /// Return the canonical media type.
    #[must_use]
    pub const fn media_type(self) -> Option<&'static str> {
        match self {
            Self::Png => Some("image/png"),
            Self::Jpeg => Some("image/jpeg"),
            Self::Gif => Some("image/gif"),
            Self::Webp => Some("image/webp"),
            Self::Mp4 => Some("video/mp4"),
            Self::Webm => Some("video/webm"),
            Self::Unknown => None,
        }
    }

    /// Return true for animated-capable media formats.
    #[must_use]
    pub const fn can_be_animated(self) -> bool {
        matches!(self, Self::Gif | Self::Webp)
    }

    /// Return true for image media.
    #[must_use]
    pub const fn is_image(self) -> bool {
        matches!(self, Self::Png | Self::Jpeg | Self::Gif | Self::Webp)
    }

    /// Return true for video media.
    #[must_use]
    pub const fn is_video(self) -> bool {
        matches!(self, Self::Mp4 | Self::Webm)
    }
}

/// Image dimensions parsed from a binary image header.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ImageDimensions {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

/// Media processing policy used by SDK preflight processors.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MediaPolicy {
    /// Optional maximum base64 encoded bytes accepted inline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_inline_base64_bytes: Option<usize>,
    /// Maximum image parts to retain, preserving newest media first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_images: Option<usize>,
    /// Maximum video parts to retain, preserving newest media first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_videos: Option<usize>,
    /// Whether GIF payloads are accepted inline.
    pub allow_gif: bool,
    /// Whether WebP payloads are accepted inline.
    pub allow_webp: bool,
    /// Whether image payloads are accepted.
    pub allow_images: bool,
    /// Whether video payloads are accepted.
    pub allow_videos: bool,
    /// Whether document/file payloads are accepted.
    pub allow_documents: bool,
}

impl Default for MediaPolicy {
    fn default() -> Self {
        Self {
            max_inline_base64_bytes: None,
            max_images: None,
            max_videos: None,
            allow_gif: true,
            allow_webp: true,
            allow_images: true,
            allow_videos: true,
            allow_documents: true,
        }
    }
}

/// Parsed data URL payload.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ParsedDataUrl {
    /// Media type from the data URL prefix.
    pub media_type: String,
    /// Decoded payload bytes.
    pub data: Vec<u8>,
}

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
            .map(raw_budget_from_base64_limit);
        let over_base64_budget = policy
            .max_inline_base64_bytes
            .is_some_and(|limit| base64_bytes > limit);
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
            budget_raw_bytes,
            allowed_by_policy: policy_reason.is_none(),
            policy_reason,
        }
    }
}

/// Detect PNG, JPEG, GIF, `WebP`, MP4, or `WebM` from magic bytes.
#[must_use]
pub fn detect_media_kind(bytes: &[u8]) -> MediaKind {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return MediaKind::Png;
    }
    if bytes.starts_with(b"\xff\xd8\xff") {
        return MediaKind::Jpeg;
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return MediaKind::Gif;
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return MediaKind::Webp;
    }
    if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        return MediaKind::Mp4;
    }
    if bytes.starts_with(b"\x1a\x45\xdf\xa3") {
        return MediaKind::Webm;
    }
    MediaKind::Unknown
}

/// Parse image dimensions from well-known image headers.
#[must_use]
pub fn detect_image_dimensions(bytes: &[u8], kind: MediaKind) -> Option<ImageDimensions> {
    match kind {
        MediaKind::Png => png_dimensions(bytes),
        MediaKind::Gif => gif_dimensions(bytes),
        MediaKind::Webp => webp_dimensions(bytes),
        MediaKind::Jpeg => jpeg_dimensions(bytes),
        MediaKind::Mp4 | MediaKind::Webm | MediaKind::Unknown => None,
    }
}

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

/// Parse a `data:<media-type>;base64,<payload>` URL.
///
/// # Errors
///
/// Returns an error when the data URL is unsupported or the payload is invalid base64.
pub fn parse_data_url(data_url: &str) -> Result<ParsedDataUrl, String> {
    let (prefix, payload) = data_url
        .split_once(',')
        .ok_or_else(|| "data URL is missing a comma separator".to_string())?;
    let media_type = prefix
        .strip_prefix("data:")
        .and_then(|value| value.strip_suffix(";base64"))
        .ok_or_else(|| "only base64 data URLs are supported".to_string())?;
    let data = STANDARD
        .decode(payload)
        .map_err(|error| format!("invalid base64 data URL payload: {error}"))?;
    Ok(ParsedDataUrl {
        media_type: media_type.to_string(),
        data,
    })
}

/// Compute base64 encoded length without line wrapping.
#[must_use]
pub const fn base64_encoded_len(raw_bytes: usize) -> usize {
    raw_bytes.div_ceil(3) * 4
}

/// Return the largest raw byte count that fits in a base64 encoded byte budget.
#[must_use]
pub const fn raw_budget_from_base64_limit(base64_limit: usize) -> usize {
    (base64_limit / 4) * 3
}

fn media_policy_reason(
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

fn png_dimensions(bytes: &[u8]) -> Option<ImageDimensions> {
    if bytes.len() < 24 || !bytes.starts_with(b"\x89PNG\r\n\x1a\n") || &bytes[12..16] != b"IHDR" {
        return None;
    }
    Some(ImageDimensions {
        width: u32::from_be_bytes(bytes[16..20].try_into().ok()?),
        height: u32::from_be_bytes(bytes[20..24].try_into().ok()?),
    })
}

fn gif_dimensions(bytes: &[u8]) -> Option<ImageDimensions> {
    if bytes.len() < 10 || !(bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a")) {
        return None;
    }
    Some(ImageDimensions {
        width: u16::from_le_bytes(bytes[6..8].try_into().ok()?).into(),
        height: u16::from_le_bytes(bytes[8..10].try_into().ok()?).into(),
    })
}

fn webp_dimensions(bytes: &[u8]) -> Option<ImageDimensions> {
    if bytes.len() < 16 || !bytes.starts_with(b"RIFF") || &bytes[8..12] != b"WEBP" {
        return None;
    }
    match &bytes[12..16] {
        b"VP8X" if bytes.len() >= 30 => Some(ImageDimensions {
            width: read_24_le(&bytes[24..27])? + 1,
            height: read_24_le(&bytes[27..30])? + 1,
        }),
        b"VP8L" if bytes.len() >= 25 => {
            let b0 = u32::from(bytes[21]);
            let b1 = u32::from(bytes[22]);
            let b2 = u32::from(bytes[23]);
            let b3 = u32::from(bytes[24]);
            Some(ImageDimensions {
                width: 1 + (((b1 & 0x3f) << 8) | b0),
                height: 1 + (((b3 & 0x0f) << 10) | (b2 << 2) | ((b1 & 0xc0) >> 6)),
            })
        }
        b"VP8 " if bytes.len() >= 30 && &bytes[23..26] == b"\x9d\x01\x2a" => {
            Some(ImageDimensions {
                width: u32::from(u16::from_le_bytes(bytes[26..28].try_into().ok()?)) & 0x3fff,
                height: u32::from(u16::from_le_bytes(bytes[28..30].try_into().ok()?)) & 0x3fff,
            })
        }
        _ => None,
    }
}

fn jpeg_dimensions(bytes: &[u8]) -> Option<ImageDimensions> {
    if bytes.len() < 4 || !bytes.starts_with(b"\xff\xd8") {
        return None;
    }
    let mut index = 2;
    while index + 9 < bytes.len() {
        if bytes[index] != 0xff {
            index += 1;
            continue;
        }
        while index < bytes.len() && bytes[index] == 0xff {
            index += 1;
        }
        if index >= bytes.len() {
            return None;
        }
        let marker = bytes[index];
        index += 1;
        if marker == 0xd8 || marker == 0xd9 || (0xd0..=0xd7).contains(&marker) {
            continue;
        }
        if index + 2 > bytes.len() {
            return None;
        }
        let segment_len = usize::from(u16::from_be_bytes(bytes[index..index + 2].try_into().ok()?));
        if segment_len < 2 || index + segment_len > bytes.len() {
            return None;
        }
        if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) {
            if segment_len < 7 {
                return None;
            }
            return Some(ImageDimensions {
                height: u32::from(u16::from_be_bytes(
                    bytes[index + 3..index + 5].try_into().ok()?,
                )),
                width: u32::from(u16::from_be_bytes(
                    bytes[index + 5..index + 7].try_into().ok()?,
                )),
            });
        }
        index += segment_len;
    }
    None
}

fn read_24_le(bytes: &[u8]) -> Option<u32> {
    if bytes.len() < 3 {
        return None;
    }
    Some(u32::from(bytes[0]) | (u32::from(bytes[1]) << 8) | (u32::from(bytes[2]) << 16))
}
