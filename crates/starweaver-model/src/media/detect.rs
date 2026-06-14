//! Media magic-byte detection.

use super::MediaKind;

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
