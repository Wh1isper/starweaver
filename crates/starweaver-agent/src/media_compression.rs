//! Media compression helpers used by SDK filters and first-party tools.

use std::io::Cursor;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use image::{codecs::jpeg::JpegEncoder, imageops::FilterType, DynamicImage, ImageFormat, RgbImage};
use serde_json::{json, Value};
use starweaver_model::{detect_media_kind, raw_budget_from_base64_limit, MediaKind};

const JPEG_QUALITIES: &[u8] = &[95, 85, 75, 60, 45, 30, 20];
const RESIZE_PASSES: usize = 5;

/// Result of an image split segment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImageSegment {
    /// Segment image bytes.
    pub data: Vec<u8>,
    /// Segment media type.
    pub media_type: String,
}

/// Result of an image compression attempt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompressedImage {
    /// Image bytes after optional compression.
    pub data: Vec<u8>,
    /// Corrected media type for `data`.
    pub media_type: String,
    /// True when bytes were re-encoded.
    pub compressed: bool,
}

/// Return the raw-byte budget implied by a base64 encoded API limit.
pub const fn raw_budget_for_encoded_limit(max_encoded_bytes: usize) -> usize {
    raw_budget_from_base64_limit(max_encoded_bytes)
}

/// Encode a data URL for an inline media payload.
pub fn data_url(media_type: &str, data: &[u8]) -> String {
    format!("data:{media_type};base64,{}", STANDARD.encode(data))
}

/// Split a tall image into vertical segments with overlap.
pub fn split_image_data(
    image_bytes: &[u8],
    max_height: usize,
    overlap: usize,
    media_type: &str,
) -> Result<Vec<ImageSegment>, String> {
    let image = image::load_from_memory(image_bytes)
        .map_err(|error| format!("failed to decode image for splitting: {error}"))?;
    let width = image.width();
    let height = usize::try_from(image.height()).unwrap_or(usize::MAX);
    let normalized_media_type = normalized_image_media_type(image_bytes, media_type);
    if max_height == 0 || height <= max_height {
        return Ok(vec![ImageSegment {
            data: image_bytes.to_vec(),
            media_type: normalized_media_type,
        }]);
    }

    let format = image_format_for_media_type(&normalized_media_type).unwrap_or(ImageFormat::Png);
    let output_media_type = media_type_for_image_format(format).to_string();
    let step = max_height.saturating_sub(overlap).max(1);
    let mut segments = Vec::new();
    let mut y = 0usize;
    while y < height {
        let segment_height = max_height.min(height.saturating_sub(y));
        let segment = image.crop_imm(
            0,
            u32::try_from(y).map_err(|_| "image segment y offset exceeds u32".to_string())?,
            width,
            u32::try_from(segment_height)
                .map_err(|_| "image segment height exceeds u32".to_string())?,
        );
        let encoded = encode_segment(&segment, format)?;
        segments.push(ImageSegment {
            data: encoded,
            media_type: output_media_type.clone(),
        });
        y = y.saturating_add(step);
        if y.saturating_add(overlap) >= height {
            break;
        }
    }
    Ok(segments)
}

/// Compress image bytes so the raw payload fits the provided byte budget.
///
/// Small images are passed through with corrected
/// media type, oversized images are converted to JPEG, JPEG quality is reduced
/// progressively, and dimensions are halved when quality reduction alone is not
/// enough. Alpha images are composited onto a white background before JPEG output.
pub fn compress_image_data(
    image_bytes: &[u8],
    max_bytes: usize,
    media_type: &str,
) -> Result<CompressedImage, String> {
    let normalized_media_type = normalized_image_media_type(image_bytes, media_type);
    if max_bytes == 0 || image_bytes.len() <= max_bytes {
        return Ok(CompressedImage {
            data: image_bytes.to_vec(),
            media_type: normalized_media_type,
            compressed: false,
        });
    }

    if detect_media_kind(image_bytes) == MediaKind::Gif {
        return Ok(CompressedImage {
            data: image_bytes.to_vec(),
            media_type: normalized_media_type,
            compressed: false,
        });
    }

    let mut rgb = decode_image_as_white_composited_rgb(image_bytes)?;
    let mut smallest = encode_jpeg(&rgb, 20)?;
    for _resize_pass in 0..RESIZE_PASSES {
        for quality in JPEG_QUALITIES {
            let encoded = encode_jpeg(&rgb, *quality)?;
            if encoded.len() <= max_bytes {
                return Ok(CompressedImage {
                    data: encoded,
                    media_type: "image/jpeg".to_string(),
                    compressed: true,
                });
            }
            if encoded.len() < smallest.len() {
                smallest = encoded;
            }
        }

        let (width, height) = rgb.dimensions();
        let resized = DynamicImage::ImageRgb8(rgb).resize(
            width.saturating_div(2).max(1),
            height.saturating_div(2).max(1),
            FilterType::Lanczos3,
        );
        rgb = resized.to_rgb8();
    }

    let fallback = encode_jpeg(&rgb, 20)?;
    if fallback.len() < smallest.len() {
        smallest = fallback;
    }
    Ok(CompressedImage {
        data: smallest,
        media_type: "image/jpeg".to_string(),
        compressed: true,
    })
}

/// Compress image bytes to fit a base64 encoded API image limit.
pub fn compress_image_to_model_limit(
    image_bytes: &[u8],
    max_encoded_bytes: usize,
    media_type: &str,
) -> Result<CompressedImage, String> {
    if max_encoded_bytes == 0 {
        return Ok(CompressedImage {
            data: image_bytes.to_vec(),
            media_type: normalized_image_media_type(image_bytes, media_type),
            compressed: false,
        });
    }
    compress_image_data(
        image_bytes,
        raw_budget_for_encoded_limit(max_encoded_bytes),
        media_type,
    )
}

/// Build the standard model-limit error for still-oversized compressed images.
pub fn oversized_after_compression_message(
    original_size: usize,
    max_encoded_bytes: usize,
) -> String {
    format!(
        "<system-reminder>An image ({original_size} bytes) was removed because it could not be compressed below the {max_encoded_bytes} byte API limit (accounting for base64 encoding). If you need this image, try resizing or converting it to a smaller format first, then use the view tool again.</system-reminder>"
    )
}

/// Build the standard compression failure message.
pub fn compression_failed_message() -> String {
    "<system-reminder>An image was removed because compression failed. If the image is needed, try compressing it to a smaller size before viewing.</system-reminder>".to_string()
}

/// Compress a JSON object containing `data_url` and optional `media_type` fields.
pub fn compress_data_url_object(
    object: &mut serde_json::Map<String, Value>,
    max_encoded_bytes: usize,
) -> Result<bool, String> {
    let Some(data_url_value) = object.get("data_url").and_then(Value::as_str) else {
        return Ok(false);
    };
    let parsed = starweaver_model::parse_data_url(data_url_value)?;
    if !parsed.media_type.starts_with("image/") {
        return Ok(false);
    }
    let max_raw_bytes = raw_budget_for_encoded_limit(max_encoded_bytes);
    if parsed.data.len() <= max_raw_bytes {
        object.insert(
            "media_type".to_string(),
            json!(normalized_image_media_type(
                &parsed.data,
                &parsed.media_type
            )),
        );
        return Ok(false);
    }
    let original_size = parsed.data.len();
    let compressed = compress_image_data(&parsed.data, max_raw_bytes, &parsed.media_type)?;
    if compressed.data.len() > max_raw_bytes {
        return Err(oversized_after_compression_message(
            original_size,
            max_encoded_bytes,
        ));
    }
    object.insert(
        "data_url".to_string(),
        json!(data_url(&compressed.media_type, &compressed.data)),
    );
    object.insert("media_type".to_string(), json!(compressed.media_type));
    Ok(compressed.compressed)
}

/// Return the detected image media type when available, otherwise the declared media type.
pub fn normalized_image_media_type(image_bytes: &[u8], declared: &str) -> String {
    detect_media_kind(image_bytes)
        .media_type()
        .filter(|media_type| media_type.starts_with("image/"))
        .unwrap_or(declared)
        .to_string()
}

const fn image_format_for_media_type(media_type: &str) -> Option<ImageFormat> {
    match media_type.as_bytes() {
        b"image/png" => Some(ImageFormat::Png),
        b"image/jpeg" | b"image/jpg" => Some(ImageFormat::Jpeg),
        b"image/gif" => Some(ImageFormat::Gif),
        b"image/webp" => Some(ImageFormat::WebP),
        _ => None,
    }
}

const fn media_type_for_image_format(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Jpeg => "image/jpeg",
        ImageFormat::Gif => "image/gif",
        ImageFormat::WebP => "image/webp",
        _ => "image/png",
    }
}

fn encode_segment(segment: &DynamicImage, format: ImageFormat) -> Result<Vec<u8>, String> {
    let mut output = Cursor::new(Vec::new());
    match format {
        ImageFormat::Jpeg => DynamicImage::ImageRgb8(segment.to_rgb8())
            .write_to(&mut output, format)
            .map_err(|error| format!("failed to encode jpeg image segment: {error}"))?,
        _ => segment
            .write_to(&mut output, format)
            .map_err(|error| format!("failed to encode image segment: {error}"))?,
    }
    Ok(output.into_inner())
}

fn decode_image_as_white_composited_rgb(image_bytes: &[u8]) -> Result<RgbImage, String> {
    let image = image::load_from_memory(image_bytes)
        .map_err(|error| format!("failed to decode image for compression: {error}"))?;
    let rgba = image.to_rgba8();
    let mut rgb = RgbImage::new(rgba.width(), rgba.height());
    for (x, y, pixel) in rgba.enumerate_pixels() {
        let alpha = u16::from(pixel[3]);
        let inv_alpha = 255u16.saturating_sub(alpha);
        let red = (u16::from(pixel[0]) * alpha + 255 * inv_alpha + 127) / 255;
        let green = (u16::from(pixel[1]) * alpha + 255 * inv_alpha + 127) / 255;
        let blue = (u16::from(pixel[2]) * alpha + 255 * inv_alpha + 127) / 255;
        rgb.put_pixel(
            x,
            y,
            image::Rgb([
                u8::try_from(red.min(255)).unwrap_or(255),
                u8::try_from(green.min(255)).unwrap_or(255),
                u8::try_from(blue.min(255)).unwrap_or(255),
            ]),
        );
    }
    Ok(rgb)
}

fn encode_jpeg(image: &RgbImage, quality: u8) -> Result<Vec<u8>, String> {
    let mut output = Vec::new();
    let mut encoder = JpegEncoder::new_with_quality(&mut output, quality);
    encoder
        .encode(
            image.as_raw(),
            image.width(),
            image.height(),
            image::ColorType::Rgb8.into(),
        )
        .map_err(|error| format!("failed to encode jpeg: {error}"))?;
    Ok(output)
}
