use std::sync::Arc;

use async_trait::async_trait;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use starweaver_model::ModelProfile;
use starweaver_tools::{ToolContext, ToolError, ToolResult};

use super::{
    args::UrlArgs,
    http::{
        content_type_from_extension, fetch_http_resource, filename_extension, validate_http_url,
    },
    json_result,
};
use crate::bundles::helpers::{tool_execution_error, tool_model_retry};

/// Media URL capability flags supplied by the active model adapter or SDK configuration.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct HostMediaCapabilities {
    /// Optional model or provider identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    /// Model accepts image URL content parts.
    pub supports_image_url: bool,
    /// Model accepts video URL content parts.
    pub supports_video_url: bool,
    /// Model accepts audio URL content parts.
    pub supports_audio_url: bool,
    /// Model accepts document URL content parts.
    pub supports_document_url: bool,
}

impl HostMediaCapabilities {
    /// Create media capabilities from a model profile.
    #[must_use]
    pub const fn from_model_profile(model_id: Option<String>, profile: &ModelProfile) -> Self {
        Self {
            model_id,
            supports_image_url: profile.supports_image_input,
            supports_video_url: profile.supports_video_input,
            supports_audio_url: profile.supports_audio_input,
            supports_document_url: profile.supports_document_input,
        }
    }
}

/// Media understanding request used by injectable fallback adapters.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MediaUnderstandingRequest {
    /// Media kind: image, video, or audio.
    pub media_kind: String,
    /// Source URL or data URL.
    pub url: String,
    /// Optional focused analysis instructions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}

/// Media understanding response returned by fallback adapters.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MediaUnderstandingResponse {
    /// Whether the fallback adapter succeeded.
    pub success: bool,
    /// Media kind.
    pub media_kind: String,
    /// Source URL.
    pub url: String,
    /// Fallback model identifier.
    pub model_id: String,
    /// Textual analysis or transcript.
    pub content: String,
    /// Whether returned content was truncated.
    pub truncated: bool,
    /// Optional metadata.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub metadata: Map<String, Value>,
}

/// Injectable fallback media understanding adapter.
#[async_trait]
pub trait HostMediaUnderstandingClient: Send + Sync {
    /// Analyze or transcribe a media URL.
    async fn understand(
        &self,
        request: MediaUnderstandingRequest,
    ) -> Result<MediaUnderstandingResponse, String>;
}

/// Typed dependency wrapper for a fallback media understanding adapter.
#[derive(Clone)]
pub struct HostMediaUnderstandingClientHandle {
    pub(crate) client: Arc<dyn HostMediaUnderstandingClient>,
}

impl HostMediaUnderstandingClientHandle {
    /// Create a media understanding client handle.
    #[must_use]
    pub fn new(client: Arc<dyn HostMediaUnderstandingClient>) -> Self {
        Self { client }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum MediaKind {
    Image,
    Video,
    Audio,
    Document,
    Text,
    Unknown,
}

impl MediaKind {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Video => "video",
            Self::Audio => "audio",
            Self::Document => "document",
            Self::Text => "text",
            Self::Unknown => "unknown",
        }
    }
}

#[allow(dead_code)]
pub(super) async fn read_image(
    context: ToolContext,
    arguments: UrlArgs,
) -> Result<ToolResult, ToolError> {
    read_media(context, arguments.url, MediaKind::Image).await
}

#[allow(dead_code)]
pub(super) async fn read_video(
    context: ToolContext,
    arguments: UrlArgs,
) -> Result<ToolResult, ToolError> {
    read_media(context, arguments.url, MediaKind::Video).await
}

#[allow(dead_code)]
pub(super) async fn read_audio(
    context: ToolContext,
    arguments: UrlArgs,
) -> Result<ToolResult, ToolError> {
    read_media(context, arguments.url, MediaKind::Audio).await
}

#[allow(dead_code)]
pub(super) async fn load_media_url(
    context: ToolContext,
    arguments: UrlArgs,
) -> Result<ToolResult, ToolError> {
    let url = validate_http_url(&context, "load_media_url", &arguments.url)?;
    let mut final_url = url.to_string();
    let mut content_type = content_type_from_extension(url.path());
    let mut probe_error = None;
    let mut kind = classify_media(content_type.as_deref(), url.path());
    if kind == MediaKind::Unknown {
        match fetch_http_resource(&context, "load_media_url", &arguments.url, Method::HEAD, 0).await
        {
            Ok(resource) => {
                final_url = resource.final_url;
                content_type = resource.content_type;
                kind = classify_media(content_type.as_deref(), url.path());
            }
            Err(error) => {
                probe_error = Some(error.to_string());
            }
        }
    }
    let capabilities = context.dependency::<HostMediaCapabilities>();
    let native_supported = capabilities
        .as_ref()
        .is_some_and(|capabilities| media_capability_supported(capabilities, kind));
    let fallback = fallback_for_media(kind);
    let provider_ready = native_supported.then(|| {
        serde_json::json!({
            "type": "media_url",
            "category": kind.as_str(),
            "url": final_url,
            "media_type": content_type,
        })
    });
    Ok(ToolResult::new(serde_json::json!({
        "success": true,
        "url": arguments.url,
        "final_url": final_url,
        "content_type": content_type,
        "category": kind.as_str(),
        "native_supported": native_supported,
        "model_id": capabilities.and_then(|capabilities| capabilities.model_id.clone()),
        "provider_ready": provider_ready,
        "fallback": fallback,
        "probe_error": probe_error,
    })))
}

async fn read_media(
    context: ToolContext,
    url: String,
    media_kind: MediaKind,
) -> Result<ToolResult, ToolError> {
    validate_http_url(&context, media_kind.as_str(), &url)?;
    let capabilities = context.dependency::<HostMediaCapabilities>();
    let native_supported = capabilities
        .as_ref()
        .is_some_and(|capabilities| media_capability_supported(capabilities, media_kind));
    if let Some(handle) = context.dependency::<HostMediaUnderstandingClientHandle>() {
        let response = handle
            .client
            .understand(MediaUnderstandingRequest {
                media_kind: media_kind.as_str().to_string(),
                url,
                instructions: None,
            })
            .await
            .map_err(|error| tool_execution_error(media_kind.as_str(), error))?;
        return json_result(response, media_kind.as_str());
    }
    Err(tool_model_retry(
        media_kind.as_str(),
        format!(
            "no HostMediaUnderstandingClientHandle fallback adapter is configured for {media_kind}. Configure a fallback adapter, use load_media_url if the active model supports this media URL kind, or switch to a media-capable model. native_supported={native_supported}",
            media_kind = media_kind.as_str(),
        ),
    ))
}

pub(super) fn classify_media(content_type: Option<&str>, path_or_url: &str) -> MediaKind {
    let lower_type = content_type
        .and_then(|value| value.split(';').next())
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if lower_type.starts_with("image/") {
        return MediaKind::Image;
    }
    if lower_type.starts_with("video/") {
        return MediaKind::Video;
    }
    if lower_type.starts_with("audio/") {
        return MediaKind::Audio;
    }
    if lower_type.starts_with("text/") || lower_type.contains("html") || lower_type.contains("json")
    {
        return MediaKind::Text;
    }
    if lower_type.contains("pdf")
        || lower_type.contains("officedocument")
        || lower_type.contains("msword")
        || lower_type.contains("ms-excel")
        || lower_type.contains("ms-powerpoint")
        || lower_type.contains("epub")
    {
        return MediaKind::Document;
    }
    match filename_extension(path_or_url).as_deref() {
        Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg") => MediaKind::Image,
        Some("mp4" | "webm" | "mov" | "m4v" | "avi" | "mkv") => MediaKind::Video,
        Some("mp3" | "wav" | "ogg" | "m4a" | "flac" | "aac") => MediaKind::Audio,
        Some("pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "epub") => {
            MediaKind::Document
        }
        Some("html" | "htm" | "txt" | "md" | "json" | "xml" | "csv") => MediaKind::Text,
        _ => MediaKind::Unknown,
    }
}

const fn media_capability_supported(capabilities: &HostMediaCapabilities, kind: MediaKind) -> bool {
    match kind {
        MediaKind::Image => capabilities.supports_image_url,
        MediaKind::Video => capabilities.supports_video_url,
        MediaKind::Audio => capabilities.supports_audio_url,
        MediaKind::Document => capabilities.supports_document_url,
        MediaKind::Text | MediaKind::Unknown => false,
    }
}

fn fallback_for_media(kind: MediaKind) -> Value {
    match kind {
        MediaKind::Image => serde_json::json!({"tool": "read_image"}),
        MediaKind::Video => serde_json::json!({"tool": "read_video"}),
        MediaKind::Audio => serde_json::json!({"tool": "read_audio"}),
        MediaKind::Document => document_handoff(""),
        MediaKind::Text => serde_json::json!({"tool": "scrape"}),
        MediaKind::Unknown => serde_json::json!({"tool": "download"}),
    }
}

pub(super) fn document_handoff(url: &str) -> Value {
    serde_json::json!({
        "tool": "download",
        "url": url,
        "next_step": "run a document-conversion skill workflow with PyMuPDF4LLM for PDF or MarkItDown for Office/EPUB resources",
    })
}
