//! Media data types.

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
