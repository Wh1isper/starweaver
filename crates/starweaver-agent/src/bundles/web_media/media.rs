use std::sync::Arc;

use async_trait::async_trait;
use reqwest::{Method, Url};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use starweaver_context::{AgentContext, ToolConfig};
use starweaver_model::{ContentPart, ModelProfile, ProtocolFamily, detect_media_kind};
use starweaver_tools::{ToolContext, ToolError, ToolResult};

use super::{
    args::{ReadMediaArgs, UrlArgs},
    http::{
        content_type_from_extension, fetch_http_resource, filename_extension, validate_http_url,
    },
    json_result,
};
use crate::bundles::helpers::{tool_execution_error, tool_model_retry};
use crate::media_compression::{
    compress_image_to_model_limit, data_url, raw_budget_for_encoded_limit,
};

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
    /// Model accepts `YouTube` URLs as provider-native video content.
    #[serde(default)]
    pub supports_youtube_url: bool,
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
            supports_youtube_url: matches!(profile.protocol, ProtocolFamily::GeminiGenerateContent)
                && profile.supports_video_input,
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

    const fn title(self) -> &'static str {
        match self {
            Self::Image => "Image",
            Self::Video => "Video",
            Self::Audio => "Audio",
            Self::Document => "Document",
            Self::Text => "Text",
            Self::Unknown => "Unknown",
        }
    }
}

#[allow(dead_code)]
pub(super) async fn read_image(
    context: ToolContext,
    arguments: UrlArgs,
) -> Result<ToolResult, ToolError> {
    read_legacy_media(context, arguments.url, MediaKind::Image).await
}

#[allow(dead_code)]
pub(super) async fn read_video(
    context: ToolContext,
    arguments: UrlArgs,
) -> Result<ToolResult, ToolError> {
    read_legacy_media(context, arguments.url, MediaKind::Video).await
}

#[allow(dead_code)]
pub(super) async fn read_audio(
    context: ToolContext,
    arguments: UrlArgs,
) -> Result<ToolResult, ToolError> {
    read_legacy_media(context, arguments.url, MediaKind::Audio).await
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

#[allow(clippy::too_many_lines)]
pub(super) async fn read_media(
    context: ToolContext,
    arguments: ReadMediaArgs,
) -> Result<ToolResult, ToolError> {
    let url = validate_http_url(&context, "read_media", &arguments.url)?;
    if is_youtube_url(&url) {
        return read_youtube_url(context, arguments, url).await;
    }

    let tool_config = tool_config(&context);
    let max_fetch_bytes = media_fetch_limit(&tool_config);
    let resource = fetch_http_resource(
        &context,
        "read_media",
        &arguments.url,
        Method::GET,
        max_fetch_bytes,
    )
    .await?;
    if !(200..400).contains(&resource.status) {
        return Err(tool_execution_error(
            "read_media",
            format!(
                "HTTP status {} while reading media URL {}",
                resource.status, resource.final_url
            ),
        ));
    }
    let Some(mut body) = resource.body.clone() else {
        return Err(tool_execution_error(
            "read_media",
            format!(
                "response body was not loaded for {} after HTTP status {}",
                resource.final_url, resource.status
            ),
        ));
    };

    let (kind, mut media_type) = classify_loaded_media(&body, &resource, &arguments.url);
    if !matches!(kind, MediaKind::Image | MediaKind::Video | MediaKind::Audio) {
        return Err(tool_model_retry(
            "read_media",
            unsupported_media_message(kind, &arguments.url),
        ));
    }

    let max_inline = max_inline_bytes(&tool_config, kind);
    if u64::try_from(body.len()).map_or(true, |length| length > max_inline) {
        return Err(tool_model_retry(
            "read_media",
            format!(
                "{} URL is too large to inline ({} bytes). Maximum supported inline size is {} bytes. Use `download` to save it, then inspect a smaller or converted local file with `view`.",
                kind.title(),
                body.len(),
                max_inline,
            ),
        ));
    }

    let original_bytes = body.len();
    let mut compressed_for_model = false;
    if kind == MediaKind::Image {
        let mut image_media_type = normalized_image_media_type(&body, media_type.as_deref());
        if !is_supported_inline_image(&image_media_type) {
            return Err(tool_model_retry(
                "read_media",
                format!(
                    "unsupported image format '{image_media_type}' for {}. Supported formats: image/gif, image/jpeg, image/png, image/webp. Use `download`, convert the image to a supported format, then inspect it with `view`.",
                    arguments.url
                ),
            ));
        }
        if let Some(agent_context) = context.dependency::<AgentContext>() {
            let max_image_bytes = agent_context.model_config.max_image_bytes;
            if max_image_bytes > 0 && body.len() > raw_budget_for_encoded_limit(max_image_bytes) {
                match compress_image_to_model_limit(&body, max_image_bytes, &image_media_type) {
                    Ok(compressed) => {
                        if compressed.data.len() > raw_budget_for_encoded_limit(max_image_bytes) {
                            return Err(tool_model_retry(
                                "read_media",
                                format!(
                                    "Image URL could not be compressed below the {max_image_bytes} byte API limit after accounting for base64 encoding. Use `download`, resize or convert it to a smaller supported format, then inspect it with `view`."
                                ),
                            ));
                        }
                        body = compressed.data;
                        image_media_type = compressed.media_type;
                        compressed_for_model = compressed.compressed;
                    }
                    Err(error) => {
                        return Err(tool_model_retry(
                            "read_media",
                            format!(
                                "Image URL could not be compressed for inline model input: {error}. Use `download`, resize or convert it to a supported smaller format, then inspect it with `view`."
                            ),
                        ));
                    }
                }
            }
        }
        media_type = Some(image_media_type);
    } else if media_type.is_none() {
        return Err(tool_model_retry(
            "read_media",
            format!(
                "{} URL media type could not be determined for {}. Use `download`, inspect the file type locally, then retry with `view`.",
                kind.title(),
                arguments.url,
            ),
        ));
    }

    let media_type = media_type.unwrap_or_else(|| "application/octet-stream".to_string());
    let capabilities = context.dependency::<HostMediaCapabilities>();
    let native_supported = capabilities
        .as_ref()
        .is_some_and(|capabilities| media_capability_supported(capabilities, kind));
    let payload = data_url(&media_type, &body);
    if native_supported {
        return media_tool_result(
            &arguments,
            &resource,
            kind,
            &media_type,
            ContentPart::data_url(payload, media_type.clone()),
            capabilities.as_deref(),
            compressed_for_model,
            original_bytes,
            body.len(),
        );
    }

    if let Some(handle) = context.dependency::<HostMediaUnderstandingClientHandle>() {
        let response = handle
            .client
            .understand(MediaUnderstandingRequest {
                media_kind: kind.as_str().to_string(),
                url: payload,
                instructions: arguments.instructions.clone(),
            })
            .await
            .map_err(|error| tool_execution_error("read_media", error))?;
        return json_result(response, "read_media");
    }

    Err(tool_model_retry(
        "read_media",
        format!(
            "The active model does not advertise native support for this remote {} URL and no HostMediaUnderstandingClientHandle fallback adapter is configured. Configure a fallback adapter, switch to a media-capable model, or use `download` followed by local `view` on a supported file.",
            kind.as_str()
        ),
    ))
}

async fn read_legacy_media(
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

async fn read_youtube_url(
    context: ToolContext,
    arguments: ReadMediaArgs,
    url: Url,
) -> Result<ToolResult, ToolError> {
    let capabilities = context.dependency::<HostMediaCapabilities>();
    let native_supported = capabilities
        .as_ref()
        .is_some_and(|capabilities| capabilities.supports_youtube_url);
    if native_supported {
        return youtube_tool_result(&arguments, &url, capabilities.as_deref());
    }

    if let Some(handle) = context.dependency::<HostMediaUnderstandingClientHandle>() {
        let response = handle
            .client
            .understand(MediaUnderstandingRequest {
                media_kind: MediaKind::Video.as_str().to_string(),
                url: url.to_string(),
                instructions: arguments.instructions.clone(),
            })
            .await
            .map_err(|error| tool_execution_error("read_media", error))?;
        return json_result(response, "read_media");
    }

    Err(tool_model_retry(
        "read_media",
        "The active model does not advertise native YouTube URL support and no HostMediaUnderstandingClientHandle fallback adapter is configured. Use a direct downloadable video/audio URL, configure a fallback adapter, or provide a local downloaded file to `view`.",
    ))
}

#[allow(clippy::too_many_arguments)]
fn media_tool_result(
    arguments: &ReadMediaArgs,
    resource: &super::http::HttpResource,
    kind: MediaKind,
    media_type: &str,
    content_part: ContentPart,
    capabilities: Option<&HostMediaCapabilities>,
    compressed_for_model: bool,
    original_bytes: usize,
    inline_bytes: usize,
) -> Result<ToolResult, ToolError> {
    let mut private_metadata = Map::new();
    private_metadata.insert(
        "starweaver_tool_return_content_parts".to_string(),
        serde_json::to_value(vec![content_part])
            .map_err(|error| tool_execution_error("read_media", error))?,
    );
    private_metadata.insert(
        "starweaver_tool_return_prompt".to_string(),
        serde_json::json!(media_prompt(
            kind,
            &resource.final_url,
            arguments.instructions.as_deref()
        )),
    );
    Ok(ToolResult::new(serde_json::json!({
        "success": true,
        "url": arguments.url,
        "final_url": resource.final_url,
        "status": resource.status,
        "content_type": resource.content_type,
        "content_length": resource.content_length,
        "media_kind": kind.as_str(),
        "media_type": media_type,
        "native_supported": true,
        "model_id": capabilities.and_then(|capabilities| capabilities.model_id.clone()),
        "message": format!("The {} is attached in a provider-native media message.", kind.as_str()),
        "instructions": arguments.instructions,
        "compressed": compressed_for_model,
        "original_bytes": original_bytes,
        "inline_bytes": inline_bytes,
    }))
    .with_private_metadata(private_metadata)
    .with_model_content(serde_json::json!(format!(
        "The {} is attached in the user message.",
        kind.as_str()
    ))))
}

fn youtube_tool_result(
    arguments: &ReadMediaArgs,
    url: &Url,
    capabilities: Option<&HostMediaCapabilities>,
) -> Result<ToolResult, ToolError> {
    let mut private_metadata = Map::new();
    private_metadata.insert(
        "starweaver_tool_return_content_parts".to_string(),
        serde_json::to_value(vec![ContentPart::file_url(url.to_string(), "video/mp4")])
            .map_err(|error| tool_execution_error("read_media", error))?,
    );
    private_metadata.insert(
        "starweaver_tool_return_prompt".to_string(),
        serde_json::json!(media_prompt(
            MediaKind::Video,
            url.as_str(),
            arguments.instructions.as_deref()
        )),
    );
    Ok(ToolResult::new(serde_json::json!({
        "success": true,
        "url": arguments.url,
        "final_url": url.as_str(),
        "media_kind": "video",
        "media_type": "video/mp4",
        "youtube_url": true,
        "native_supported": true,
        "model_id": capabilities.and_then(|capabilities| capabilities.model_id.clone()),
        "message": "The YouTube URL is attached in a provider-native media message.",
        "instructions": arguments.instructions,
    }))
    .with_private_metadata(private_metadata)
    .with_model_content(serde_json::json!(
        "The video URL is attached in the user message."
    )))
}

fn classify_loaded_media(
    body: &[u8],
    resource: &super::http::HttpResource,
    requested_url: &str,
) -> (MediaKind, Option<String>) {
    let sniffed = detect_media_kind(body).media_type().map(str::to_string);
    let header = normalized_content_type(resource.content_type.as_deref());
    let final_path = Url::parse(&resource.final_url)
        .ok()
        .map_or_else(|| resource.final_url.clone(), |url| url.path().to_string());
    let extension_type = content_type_from_extension(&final_path)
        .or_else(|| content_type_from_extension(requested_url));
    let media_type = sniffed.or(header).or(extension_type);
    let kind = classify_media(media_type.as_deref(), &final_path);
    if kind == MediaKind::Unknown {
        return (
            classify_media(media_type.as_deref(), requested_url),
            media_type,
        );
    }
    (kind, media_type)
}

fn normalized_content_type(content_type: Option<&str>) -> Option<String> {
    content_type
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
}

fn normalized_image_media_type(body: &[u8], declared: Option<&str>) -> String {
    detect_media_kind(body)
        .media_type()
        .filter(|media_type| media_type.starts_with("image/"))
        .or(declared)
        .unwrap_or("application/octet-stream")
        .to_string()
}

fn is_supported_inline_image(media_type: &str) -> bool {
    matches!(
        media_type,
        "image/gif" | "image/jpeg" | "image/png" | "image/webp"
    )
}

fn tool_config(context: &ToolContext) -> ToolConfig {
    context
        .dependency::<AgentContext>()
        .map_or_else(ToolConfig::default, |context| context.tool_config.clone())
}

fn media_fetch_limit(tool_config: &ToolConfig) -> u64 {
    tool_config
        .view_max_inline_image_bytes
        .max(tool_config.view_max_inline_video_bytes)
        .max(tool_config.view_max_inline_audio_bytes)
        .max(1)
}

const fn max_inline_bytes(tool_config: &ToolConfig, kind: MediaKind) -> u64 {
    match kind {
        MediaKind::Image => tool_config.view_max_inline_image_bytes,
        MediaKind::Video => tool_config.view_max_inline_video_bytes,
        MediaKind::Audio => tool_config.view_max_inline_audio_bytes,
        MediaKind::Document | MediaKind::Text | MediaKind::Unknown => 0,
    }
}

fn is_youtube_url(url: &Url) -> bool {
    let Some(host) = url.host_str().map(str::to_ascii_lowercase) else {
        return false;
    };
    host == "youtu.be" || host == "youtube.com" || host.ends_with(".youtube.com")
}

fn media_prompt(kind: MediaKind, source: &str, instructions: Option<&str>) -> String {
    let mut prompt = format!(
        "The read_media tool loaded remote {kind} URL `{source}`. Inspect the attached media and answer accordingly.",
        kind = kind.as_str(),
    );
    if let Some(instructions) = instructions.filter(|value| !value.trim().is_empty()) {
        prompt.push_str(
            "

Analysis instructions:
",
        );
        prompt.push_str(instructions.trim());
    }
    prompt
}

fn unsupported_media_message(kind: MediaKind, url: &str) -> String {
    match kind {
        MediaKind::Document => format!(
            "URL appears to be a document rather than image, video, or audio media: {url}. Use `download`, then run a document conversion workflow or inspect the local file with `view` when supported."
        ),
        MediaKind::Text => format!(
            "URL appears to be text or web content rather than image, video, or audio media: {url}. Use `scrape` for web pages or `fetch` for text resources."
        ),
        MediaKind::Unknown => format!(
            "URL media type could not be identified as image, video, or audio: {url}. Use `download` to save the resource, inspect the local file type, then use `view` if it is supported media."
        ),
        MediaKind::Image | MediaKind::Video | MediaKind::Audio => {
            format!("unsupported media URL: {url}")
        }
    }
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
        MediaKind::Image | MediaKind::Video | MediaKind::Audio => {
            serde_json::json!({"tool": "read_media"})
        }
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
