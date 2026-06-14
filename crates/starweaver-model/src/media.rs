//! Media preflight helpers for provider-neutral model content.

mod data_url;
mod dimensions;
mod policy;
mod preflight;
mod types;

pub use data_url::{base64_encoded_len, parse_data_url, raw_budget_from_base64_limit};
pub use dimensions::detect_image_dimensions;
pub use policy::{is_document_media_type, is_image_media_type, is_video_media_type};
pub use preflight::MediaPreflight;
pub use types::{detect_media_kind, ImageDimensions, MediaKind, MediaPolicy, ParsedDataUrl};
