//! Media compression helpers used by SDK filters and first-party tools.

use std::io::Cursor;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use image::{
    AnimationDecoder, DynamicImage, ImageDecoder, ImageFormat, ImageReader, RgbImage,
    codecs::gif::GifDecoder, codecs::jpeg::JpegEncoder, codecs::webp::WebPDecoder,
    imageops::FilterType,
};
use serde_json::{Value, json};
use starweaver_model::{
    MediaKind, detect_image_dimensions, detect_media_kind, raw_budget_from_base64_limit,
};

const JPEG_QUALITIES: &[u8] = &[95, 85, 75, 60, 45, 30, 20];
const RESIZE_PASSES: usize = 5;
const MAX_IMAGE_PROCESSING_PIXELS: u64 = 8_000_000;
const MAX_IMAGE_DECODER_ALLOC_BYTES: u64 = 64 * 1024 * 1024;

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

/// Return whether image bytes exceed either active model input limit.
///
/// `max_encoded_bytes` is the base64-encoded API budget. A zero value disables
/// that limit. `max_dimension` is the maximum width or height in pixels and is
/// independently disabled by zero. Unreadable dimensions fail closed whenever
/// the dimension limit is active.
#[must_use]
pub fn image_exceeds_model_limits(
    image_bytes: &[u8],
    max_encoded_bytes: usize,
    max_dimension: usize,
) -> bool {
    if max_encoded_bytes > 0 && image_bytes.len() > raw_budget_for_encoded_limit(max_encoded_bytes)
    {
        return true;
    }
    if max_dimension == 0 {
        return false;
    }
    let kind = detect_media_kind(image_bytes);
    detect_image_dimensions(image_bytes, kind).is_none_or(|dimensions| {
        usize::try_from(dimensions.width).unwrap_or(usize::MAX) > max_dimension
            || usize::try_from(dimensions.height).unwrap_or(usize::MAX) > max_dimension
    })
}

/// Return whether image bytes satisfy both active model input limits.
#[must_use]
pub fn image_within_model_limits(
    image_bytes: &[u8],
    max_encoded_bytes: usize,
    max_dimension: usize,
) -> bool {
    !image_exceeds_model_limits(image_bytes, max_encoded_bytes, max_dimension)
}

/// Split a tall image into vertical segments with overlap.
pub fn split_image_data(
    image_bytes: &[u8],
    max_height: usize,
    overlap: usize,
    media_type: &str,
) -> Result<Vec<ImageSegment>, String> {
    let normalized_media_type = normalized_image_media_type(image_bytes, media_type);
    let kind = detect_media_kind(image_bytes);
    let dimensions = detect_image_dimensions(image_bytes, kind);
    let height = dimensions.map_or(usize::MAX, |dimensions| {
        usize::try_from(dimensions.height).unwrap_or(usize::MAX)
    });
    if max_height == 0 || height <= max_height {
        return Ok(vec![ImageSegment {
            data: image_bytes.to_vec(),
            media_type: normalized_media_type,
        }]);
    }
    safe_processing_dimensions(dimensions)?;
    if image_is_animated(image_bytes, kind)? {
        return Ok(vec![ImageSegment {
            data: image_bytes.to_vec(),
            media_type: normalized_media_type,
        }]);
    }
    let image = decode_image_for_processing(image_bytes, "splitting")?;
    let width = image.width();

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

/// Process image bytes so they fit optional raw-byte and per-axis limits.
///
/// Images already satisfying both limits are passed through with a corrected
/// media type. Images requiring processing are resized proportionally when a
/// dimension is over the limit, converted to JPEG, reduced in quality, and
/// finally reduced in dimensions when needed. Alpha is composited onto white.
/// Animated GIF/WebP payloads are returned unchanged so callers can reject an
/// over-limit result without silently dropping animation frames.
pub fn compress_image_data(
    image_bytes: &[u8],
    max_bytes: Option<usize>,
    max_dimension: usize,
    media_type: &str,
) -> Result<CompressedImage, String> {
    let normalized_media_type = normalized_image_media_type(image_bytes, media_type);
    let kind = detect_media_kind(image_bytes);
    let dimensions = detect_image_dimensions(image_bytes, kind);
    let exceeds_bytes = max_bytes.is_some_and(|limit| image_bytes.len() > limit);
    let exceeds_dimensions = max_dimension > 0
        && dimensions.is_none_or(|dimensions| {
            usize::try_from(dimensions.width).unwrap_or(usize::MAX) > max_dimension
                || usize::try_from(dimensions.height).unwrap_or(usize::MAX) > max_dimension
        });
    if !exceeds_bytes && !exceeds_dimensions {
        return Ok(CompressedImage {
            data: image_bytes.to_vec(),
            media_type: normalized_media_type,
            compressed: false,
        });
    }

    safe_processing_dimensions(dimensions)?;

    if image_is_animated(image_bytes, kind)? {
        return Ok(CompressedImage {
            data: image_bytes.to_vec(),
            media_type: normalized_media_type,
            compressed: false,
        });
    }

    let mut image = decode_image_for_processing(image_bytes, "compression")?;
    if exceeds_dimensions {
        let maximum = u32::try_from(max_dimension).unwrap_or(u32::MAX);
        image = image.resize(maximum, maximum, FilterType::Lanczos3);
    }
    let mut rgb = white_composited_rgb(image);
    let mut smallest = encode_jpeg(&rgb, 20)?;
    for _resize_pass in 0..RESIZE_PASSES {
        for quality in JPEG_QUALITIES {
            let encoded = encode_jpeg(&rgb, *quality)?;
            if max_bytes.is_none_or(|limit| encoded.len() <= limit) {
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

/// Process image bytes to fit base64 encoded API and per-axis limits.
pub fn compress_image_to_model_limit(
    image_bytes: &[u8],
    max_encoded_bytes: usize,
    max_dimension: usize,
    media_type: &str,
) -> Result<CompressedImage, String> {
    let max_bytes =
        (max_encoded_bytes > 0).then(|| raw_budget_for_encoded_limit(max_encoded_bytes));
    compress_image_data(image_bytes, max_bytes, max_dimension, media_type)
}

/// Build the standard model-limit error for still-oversized processed images.
pub fn oversized_after_compression_message(
    original_size: usize,
    max_encoded_bytes: usize,
    max_dimension: usize,
) -> String {
    let mut limits = Vec::with_capacity(2);
    if max_encoded_bytes > 0 {
        limits.push(format!(
            "the {max_encoded_bytes} byte API limit after accounting for base64 encoding"
        ));
    }
    if max_dimension > 0 {
        limits.push(format!("the {max_dimension} pixel maximum image dimension"));
    }
    let limits = if limits.is_empty() {
        "the configured model image limits".to_string()
    } else {
        limits.join(" and ")
    };
    format!(
        "<system-reminder>An image ({original_size} bytes) was removed because it could not be processed within {limits}. If you need this image, try resizing or converting it to a smaller format first, then use the view tool again.</system-reminder>"
    )
}

/// Build the standard image processing failure message.
pub fn compression_failed_message() -> String {
    "<system-reminder>An image was removed because model-limit processing failed. If the image is needed, try resizing or converting it to a smaller supported format before viewing.</system-reminder>".to_string()
}

/// Process a JSON object containing `data_url` and optional `media_type` fields.
pub fn compress_data_url_object(
    object: &mut serde_json::Map<String, Value>,
    max_encoded_bytes: usize,
    max_dimension: usize,
) -> Result<bool, String> {
    let Some(data_url_value) = object.get("data_url").and_then(Value::as_str) else {
        return Ok(false);
    };
    let parsed = starweaver_model::parse_data_url(data_url_value)?;
    if !(parsed.media_type.starts_with("image/") || detect_media_kind(&parsed.data).is_image()) {
        return Ok(false);
    }
    if image_within_model_limits(&parsed.data, max_encoded_bytes, max_dimension) {
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
    let compressed = compress_image_to_model_limit(
        &parsed.data,
        max_encoded_bytes,
        max_dimension,
        &parsed.media_type,
    )?;
    if !image_within_model_limits(&compressed.data, max_encoded_bytes, max_dimension) {
        return Err(oversized_after_compression_message(
            original_size,
            max_encoded_bytes,
            max_dimension,
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

fn safe_processing_dimensions(
    dimensions: Option<starweaver_model::ImageDimensions>,
) -> Result<starweaver_model::ImageDimensions, String> {
    let dimensions = dimensions.ok_or_else(|| {
        "image dimensions could not be validated for model-limit processing".to_string()
    })?;
    let pixel_count = u64::from(dimensions.width)
        .checked_mul(u64::from(dimensions.height))
        .ok_or_else(|| "image pixel count overflowed during safety validation".to_string())?;
    if pixel_count > MAX_IMAGE_PROCESSING_PIXELS {
        return Err(format!(
            "image has {pixel_count} pixels, exceeding the safe processing limit of {MAX_IMAGE_PROCESSING_PIXELS} pixels"
        ));
    }
    Ok(dimensions)
}

fn image_decoder_limits() -> image::Limits {
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(MAX_IMAGE_DECODER_ALLOC_BYTES);
    limits
}

fn decode_image_for_processing(
    image_bytes: &[u8],
    operation: &str,
) -> Result<DynamicImage, String> {
    let mut reader = ImageReader::new(Cursor::new(image_bytes))
        .with_guessed_format()
        .map_err(|error| format!("failed to inspect image format for {operation}: {error}"))?;
    reader.limits(image_decoder_limits());
    reader
        .decode()
        .map_err(|error| format!("failed to decode image for {operation}: {error}"))
}

fn image_is_animated(image_bytes: &[u8], kind: MediaKind) -> Result<bool, String> {
    match kind {
        MediaKind::Gif => {
            let mut decoder = GifDecoder::new(Cursor::new(image_bytes))
                .map_err(|error| format!("failed to inspect gif animation frames: {error}"))?;
            decoder
                .set_limits(image_decoder_limits())
                .map_err(|error| format!("gif animation exceeds decoder limits: {error}"))?;
            let mut frames = decoder.into_frames();
            let first = frames
                .next()
                .transpose()
                .map_err(|error| format!("failed to inspect gif animation frames: {error}"))?;
            drop(first);
            frames
                .next()
                .transpose()
                .map(|frame| frame.is_some())
                .map_err(|error| format!("failed to inspect gif animation frames: {error}"))
        }
        MediaKind::Webp => {
            let decoder = WebPDecoder::new(Cursor::new(image_bytes))
                .map_err(|error| format!("failed to inspect webp animation metadata: {error}"))?;
            Ok(decoder.has_animation())
        }
        MediaKind::Png
        | MediaKind::Jpeg
        | MediaKind::Mp4
        | MediaKind::Webm
        | MediaKind::Unknown => Ok(false),
    }
}

fn white_composited_rgb(image: DynamicImage) -> RgbImage {
    let rgba = image.into_rgba8();
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
    rgb
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_preserves_animated_gif_instead_of_staticizing_frames() -> Result<(), String> {
        let mut gif = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut gif);
            let first = image::Frame::new(image::RgbaImage::from_pixel(
                2,
                4,
                image::Rgba([255, 0, 0, 255]),
            ));
            let second = image::Frame::new(image::RgbaImage::from_pixel(
                2,
                4,
                image::Rgba([0, 0, 255, 255]),
            ));
            encoder
                .encode_frames([first, second])
                .map_err(|error| format!("failed to encode animated gif: {error}"))?;
        }

        let segments = split_image_data(&gif, 2, 0, "image/gif")?;

        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].data, gif);
        assert_eq!(segments[0].media_type, "image/gif");
        Ok(())
    }

    #[test]
    fn processing_rejects_oversized_pixel_count_before_decode() {
        let mut png = vec![0; 24];
        png[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        png[12..16].copy_from_slice(b"IHDR");
        png[16..20].copy_from_slice(&3_000u32.to_be_bytes());
        png[20..24].copy_from_slice(&3_000u32.to_be_bytes());

        let result = compress_image_data(&png, Some(1), 0, "image/png");
        let expected = format!("safe processing limit of {MAX_IMAGE_PROCESSING_PIXELS}");

        assert!(matches!(result, Err(error) if error.contains(&expected)));
    }
}
