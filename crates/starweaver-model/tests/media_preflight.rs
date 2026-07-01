//! Media preflight and canonical content part tests.

use serde_json::json;
use starweaver_model::{
    ContentPart, ImageDimensions, MediaKind, MediaPolicy, MediaPreflight, base64_encoded_len,
    detect_image_dimensions, detect_media_kind, parse_data_url, raw_budget_from_base64_limit,
};

#[test]
fn detects_common_image_and_video_magic_bytes() {
    assert_eq!(detect_media_kind(b"\x89PNG\r\n\x1a\nrest"), MediaKind::Png);
    assert_eq!(detect_media_kind(b"\xff\xd8\xff\xe0rest"), MediaKind::Jpeg);
    assert_eq!(detect_media_kind(b"GIF89arest"), MediaKind::Gif);
    assert_eq!(
        detect_media_kind(b"RIFF\x01\x00\x00\x00WEBPrest"),
        MediaKind::Webp
    );
    assert_eq!(detect_media_kind(b"\0\0\0\x18ftypmp42rest"), MediaKind::Mp4);
    assert_eq!(detect_media_kind(b"\x1a\x45\xdf\xa3rest"), MediaKind::Webm);
    assert_eq!(detect_media_kind(b"plain"), MediaKind::Unknown);
}

#[test]
fn corrects_declared_media_type_from_detected_bytes() {
    let preflight = MediaPreflight::inspect(&png_bytes(2, 1), Some("image/jpeg"));
    assert_eq!(preflight.detected_kind, MediaKind::Png);
    assert_eq!(preflight.corrected_media_type.as_deref(), Some("image/png"));
    assert!(preflight.media_type_corrected);
    assert_eq!(preflight.raw_bytes, png_bytes(2, 1).len());
    assert_eq!(
        preflight.dimensions,
        Some(ImageDimensions {
            width: 2,
            height: 1
        })
    );
    assert!(!preflight.corrupt);
}

#[test]
fn flags_corrupt_and_policy_rejected_media() {
    let corrupt = MediaPreflight::inspect(b"GIF89a", Some("image/gif"));
    assert!(corrupt.corrupt);
    assert!(
        corrupt
            .corruption_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("logical screen"))
    );

    let policy = MediaPolicy {
        allow_gif: false,
        ..MediaPolicy::default()
    };
    let rejected =
        MediaPreflight::inspect_with_policy(&gif_bytes(1, 1), Some("image/gif"), &policy);
    assert!(!rejected.allowed_by_policy);
    assert_eq!(
        rejected.policy_reason.as_deref(),
        Some("gif media is disabled by policy")
    );
}

#[test]
fn computes_base64_raw_budget_and_over_budget_flag() {
    assert_eq!(base64_encoded_len(0), 0);
    assert_eq!(base64_encoded_len(1), 4);
    assert_eq!(base64_encoded_len(2), 4);
    assert_eq!(base64_encoded_len(3), 4);
    assert_eq!(base64_encoded_len(4), 8);
    assert_eq!(raw_budget_from_base64_limit(8), 6);

    let policy = MediaPolicy {
        max_inline_base64_bytes: Some(4),
        ..MediaPolicy::default()
    };
    let preflight =
        MediaPreflight::inspect_with_policy(&png_bytes(1, 1), Some("image/png"), &policy);
    assert!(preflight.over_base64_budget);
    assert_eq!(preflight.budget_raw_bytes, Some(3));
}

#[test]
fn parses_data_url_and_image_dimensions() -> Result<(), String> {
    let parsed = parse_data_url("data:image/png;base64,iVBORw0KGgo=")?;
    assert_eq!(parsed.media_type, "image/png");
    assert_eq!(parsed.data, b"\x89PNG\r\n\x1a\n");

    assert_eq!(
        detect_image_dimensions(&png_bytes(640, 480), MediaKind::Png),
        Some(ImageDimensions {
            width: 640,
            height: 480
        })
    );
    assert_eq!(
        detect_image_dimensions(&gif_bytes(16, 9), MediaKind::Gif),
        Some(ImageDimensions {
            width: 16,
            height: 9
        })
    );
    assert_eq!(
        detect_image_dimensions(&jpeg_bytes(32, 24), MediaKind::Jpeg),
        Some(ImageDimensions {
            width: 32,
            height: 24
        })
    );
    Ok(())
}

#[test]
fn binary_and_resource_parts_serialize_with_stable_tags() -> Result<(), serde_json::Error> {
    let binary = ContentPart::Binary {
        data: vec![1, 2, 3],
        media_type: "image/png".to_string(),
    };
    let value = serde_json::to_value(&binary)?;
    assert_eq!(value["kind"], "binary");
    assert_eq!(value["media_type"], "image/png");

    let resource = ContentPart::ResourceRef {
        uri: "resource://image/1".to_string(),
        media_type: "image/png".to_string(),
        resource_type: "image".to_string(),
        metadata: serde_json::Map::from_iter([("sha256".to_string(), json!("abc"))]),
    };
    let value = serde_json::to_value(&resource)?;
    assert_eq!(value["kind"], "resource_ref");
    assert_eq!(value["uri"], "resource://image/1");
    Ok(())
}

fn png_bytes(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    bytes.extend_from_slice(&13u32.to_be_bytes());
    bytes.extend_from_slice(b"IHDR");
    bytes.extend_from_slice(&width.to_be_bytes());
    bytes.extend_from_slice(&height.to_be_bytes());
    bytes.extend_from_slice(&[8, 2, 0, 0, 0]);
    bytes.extend_from_slice(&0u32.to_be_bytes());
    bytes.extend_from_slice(&0u32.to_be_bytes());
    bytes.extend_from_slice(b"IEND");
    bytes.extend_from_slice(&0u32.to_be_bytes());
    bytes
}

fn gif_bytes(width: u16, height: u16) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"GIF89a");
    bytes.extend_from_slice(&width.to_le_bytes());
    bytes.extend_from_slice(&height.to_le_bytes());
    bytes.extend_from_slice(&[0, 0, 0]);
    bytes
}

fn jpeg_bytes(width: u16, height: u16) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"\xff\xd8");
    bytes.extend_from_slice(b"\xff\xc0");
    bytes.extend_from_slice(&17u16.to_be_bytes());
    bytes.push(8);
    bytes.extend_from_slice(&height.to_be_bytes());
    bytes.extend_from_slice(&width.to_be_bytes());
    bytes.extend_from_slice(&[3, 1, 0, 2, 0, 3, 0, 0, 0, 0]);
    bytes.extend_from_slice(b"\xff\xd9");
    bytes
}
