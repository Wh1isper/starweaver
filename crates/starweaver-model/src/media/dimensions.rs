//! Image dimension parsing from binary headers.

use super::{ImageDimensions, MediaKind};

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
